//! Shared application state and the background file watcher.
//!
//! The watcher observes the *parent directories* of the configured files
//! (Foxhole rewrites saves atomically, so watching the directory is more
//! reliable than watching the file inode). Filesystem events are filtered down
//! to the configured files and debounced with a quiet period: each event resets
//! the file's timer, and the pipeline only runs once a file has been left
//! untouched for [`DEBOUNCE_PERIOD`]. Each result is emitted to the frontend as
//! a `sync-status` event.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use notify::{Config as NotifyConfig, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use serde::Serialize;
use tauri::{AppHandle, Emitter};

use crate::config::{self, Config};
use crate::dedup::{self, SyncMemory};
use crate::pipeline;
use crate::sync::{self, UpdatePayload};

/// How long a file must stay untouched before it is processed. Every new
/// modification within this window restarts the countdown.
const DEBOUNCE_PERIOD: Duration = Duration::from_secs(3);

/// How often the debounce worker checks whether any file's quiet period has
/// elapsed. Bounds the extra latency added on top of [`DEBOUNCE_PERIOD`].
const POLL_INTERVAL: Duration = Duration::from_millis(250);

/// Maps a watched file to the instant of its most recent filesystem event.
type Pending = Arc<Mutex<HashMap<PathBuf, Instant>>>;

/// Event emitted to the frontend after each sync attempt.
#[derive(Debug, Clone, Serialize)]
pub struct SyncStatus {
    /// The `.sav` file involved.
    pub file: String,
    /// Whether the sync succeeded.
    pub ok: bool,
    /// Human-readable detail (entry count on success, error on failure).
    pub message: String,
    /// Number of extracted entries, when known.
    pub entries: Option<usize>,
    /// Event timestamp (RFC-3339).
    pub at: String,
}

/// State shared with the watcher and debounce threads.
struct Shared {
    config: Config,
    /// Remembered `LastUpdated` per stockpile, used to drop unchanged entries
    /// from outgoing payloads.
    memory: SyncMemory,
}

/// The watcher handle plus the directories it is currently watching.
struct WatcherState {
    watcher: Option<RecommendedWatcher>,
    watched_dirs: HashSet<PathBuf>,
}

/// Application state, managed by Tauri and shared across commands.
pub struct AppState {
    app: AppHandle,
    client: reqwest::blocking::Client,
    shared: Arc<Mutex<Shared>>,
    watcher: Mutex<WatcherState>,
    /// Quiet-period timers, shared with the watcher callback and worker thread.
    pending: Pending,
}

impl AppState {
    /// Build state from the persisted config, start watching its files, and
    /// spawn the debounce worker.
    pub fn new(app: AppHandle) -> Self {
        let config = config::load(&app);
        let client = reqwest::blocking::Client::builder()
            .user_agent(concat!("foxlogi-stockpiler/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_else(|_| reqwest::blocking::Client::new());

        let memory = dedup::load(&app);
        let shared = Arc::new(Mutex::new(Shared { config, memory }));
        let pending: Pending = Arc::new(Mutex::new(HashMap::new()));

        let watcher = build_watcher(shared.clone(), pending.clone());
        let mut watcher_state = WatcherState {
            watcher,
            watched_dirs: HashSet::new(),
        };

        // Begin watching the directories of any already-configured files.
        let files = shared.lock().unwrap().config.files.clone();
        sync_watched_dirs(&mut watcher_state, &files);

        // The worker fires the pipeline once a file's quiet period elapses.
        spawn_debounce_worker(app.clone(), client.clone(), shared.clone(), pending.clone());

        Self {
            app,
            client,
            shared,
            watcher: Mutex::new(watcher_state),
            pending,
        }
    }

    /// A snapshot of the current config.
    pub fn config(&self) -> Config {
        self.shared.lock().unwrap().config.clone()
    }

    /// Apply a mutation to the config, persist it, and re-sync the watcher to
    /// the (possibly changed) file set. Returns the updated config.
    pub fn update_config<F>(&self, mutate: F) -> Result<Config, String>
    where
        F: FnOnce(&mut Config),
    {
        let updated = {
            let mut shared = self.shared.lock().unwrap();
            mutate(&mut shared.config);
            shared.config.clone()
        };

        config::save(&self.app, &updated)?;

        {
            let mut watcher_state = self.watcher.lock().unwrap();
            sync_watched_dirs(&mut watcher_state, &updated.files);
        }

        Ok(updated)
    }

    /// Manually run the pipeline for one file right now, bypassing the debounce.
    /// Any pending quiet-period timer for the file is cleared so it won't also
    /// fire. Returns the entry count on success.
    ///
    /// Unlike the background watcher, this does *not* emit a `sync-status`
    /// event: the result is returned straight to the caller (the `sync_now`
    /// command), which logs it. Emitting here too would double-log every manual
    /// sync in the activity feed.
    pub fn sync_file(&self, path: &Path) -> Result<usize, String> {
        self.pending.lock().unwrap().remove(&canonical(path));

        let (base_url, api_key) = endpoint_settings(&self.shared);
        let result =
            process_with_dedup(&self.app, &self.client, &self.shared, &base_url, &api_key, path);
        result
            .map(|payload| payload.data.as_array().map(Vec::len).unwrap_or(0))
            .map_err(|e| e.to_string())
    }
}

/// Construct the watcher with a closure that records filesystem events. The
/// closure does no work beyond resetting the relevant file's quiet-period
/// timer; the worker thread performs the actual sync.
fn build_watcher(shared: Arc<Mutex<Shared>>, pending: Pending) -> Option<RecommendedWatcher> {
    let handler = move |result: notify::Result<Event>| {
        let event = match result {
            Ok(event) => event,
            Err(err) => {
                log::warn!("watch error: {err}");
                return;
            }
        };

        if !matches!(event.kind, EventKind::Modify(_) | EventKind::Create(_)) {
            return;
        }

        for path in event.paths {
            note_change(&shared, &pending, &path);
        }
    };

    match RecommendedWatcher::new(handler, NotifyConfig::default()) {
        Ok(watcher) => Some(watcher),
        Err(err) => {
            log::error!("failed to create file watcher: {err}");
            None
        }
    }
}

/// Ensure the watcher is observing the parent directory of every configured
/// file (and only watches each directory once).
fn sync_watched_dirs(state: &mut WatcherState, files: &[String]) {
    let Some(watcher) = state.watcher.as_mut() else {
        return;
    };

    let wanted: HashSet<PathBuf> = files
        .iter()
        .filter_map(|f| Path::new(f).parent().map(Path::to_path_buf))
        .collect();

    // Watch newly-wanted directories.
    for dir in &wanted {
        if state.watched_dirs.contains(dir) {
            continue;
        }
        match watcher.watch(dir, RecursiveMode::NonRecursive) {
            Ok(()) => {
                state.watched_dirs.insert(dir.clone());
            }
            Err(err) => log::warn!("could not watch {}: {err}", dir.display()),
        }
    }

    // Stop watching directories no longer needed.
    let stale: Vec<PathBuf> = state
        .watched_dirs
        .iter()
        .filter(|dir| !wanted.contains(*dir))
        .cloned()
        .collect();
    for dir in stale {
        let _ = watcher.unwatch(&dir);
        state.watched_dirs.remove(&dir);
    }
}

/// Record a filesystem event: if `path` is a watched file, (re)start its
/// quiet-period timer. This is the trailing edge of the debounce — repeated
/// writes keep pushing the deadline out.
fn note_change(shared: &Arc<Mutex<Shared>>, pending: &Pending, path: &Path) {
    let matched = {
        let shared = shared.lock().unwrap();
        shared
            .config
            .files
            .iter()
            .any(|f| same_file(Path::new(f), path))
    };
    if !matched {
        return;
    }

    // Key by canonical path so different spellings of the same file share one
    // timer.
    pending
        .lock()
        .unwrap()
        .insert(canonical(path), Instant::now());
}

/// Background loop: every [`POLL_INTERVAL`], process any file that has now been
/// untouched for at least [`DEBOUNCE_PERIOD`].
fn spawn_debounce_worker(
    app: AppHandle,
    client: reqwest::blocking::Client,
    shared: Arc<Mutex<Shared>>,
    pending: Pending,
) {
    std::thread::spawn(move || loop {
        std::thread::sleep(POLL_INTERVAL);
        let now = Instant::now();

        // Take the files whose quiet period has elapsed, removing them so they
        // aren't processed again until the next change.
        let ready: Vec<PathBuf> = {
            let mut pending = pending.lock().unwrap();
            let ready: Vec<PathBuf> = pending
                .iter()
                .filter(|(_, last_event)| now.duration_since(**last_event) >= DEBOUNCE_PERIOD)
                .map(|(path, _)| path.clone())
                .collect();
            for path in &ready {
                pending.remove(path);
            }
            ready
        };

        let (base_url, api_key) = if ready.is_empty() {
            continue;
        } else {
            endpoint_settings(&shared)
        };

        for path in ready {
            let result = process_with_dedup(&app, &client, &shared, &base_url, &api_key, &path);
            emit_status(&app, &path, &result);
        }
    });
}

/// Build a file's payload, drop entries whose `LastUpdated` is unchanged since
/// the last successful send, POST what remains, and remember the sent values.
///
/// Filtering runs against the shared [`SyncMemory`], which is updated *only
/// after* a successful POST — so a failed request never causes an entry to be
/// skipped next time. When every entry is unchanged the POST is skipped
/// entirely and a payload with an empty `data` list is returned.
fn process_with_dedup(
    app: &AppHandle,
    client: &reqwest::blocking::Client,
    shared: &Arc<Mutex<Shared>>,
    base_url: &str,
    api_key: &str,
    path: &Path,
) -> Result<UpdatePayload, pipeline::PipelineError> {
    let mut payload = pipeline::build_payload(path)?;

    let items = match payload.data.take() {
        serde_json::Value::Array(items) => items,
        other => {
            // Defensive: `extract` always yields an array. Send as-is, no dedup.
            payload.data = other;
            sync::post_update(client, base_url, api_key, &payload)?;
            return Ok(payload);
        }
    };

    let selection = {
        let shared = shared.lock().unwrap();
        shared.memory.filter(&payload.filename, items)
    };

    if selection.kept.is_empty() {
        // Nothing changed since the last sync — don't bother the server.
        payload.data = serde_json::Value::Array(Vec::new());
        return Ok(payload);
    }

    payload.data = serde_json::Value::Array(selection.kept);
    sync::post_update(client, base_url, api_key, &payload)?;

    // The send succeeded: remember these LastUpdated values (so the same
    // entries are skipped next time) and persist the memory to disk.
    let memory = {
        let mut shared = shared.lock().unwrap();
        shared.memory.commit(&payload.filename, selection.updates);
        shared.memory.clone()
    };
    if let Err(err) = dedup::save(app, &memory) {
        log::warn!("could not persist sync memory: {err}");
    }

    Ok(payload)
}

/// Read the current server base URL and API key from the config.
fn endpoint_settings(shared: &Arc<Mutex<Shared>>) -> (String, String) {
    let shared = shared.lock().unwrap();
    (
        shared
            .config
            .server_url
            .clone()
            .unwrap_or_else(|| sync::DEFAULT_SERVER_BASE.to_string()),
        shared.config.api_key.clone(),
    )
}

/// Emit a `sync-status` event to the frontend describing the outcome.
fn emit_status(
    app: &AppHandle,
    path: &Path,
    result: &Result<UpdatePayload, pipeline::PipelineError>,
) {
    let file = path
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("unknown.sav")
        .to_string();
    let at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);

    let status = match result {
        Ok(payload) => {
            let entries = payload.data.as_array().map(Vec::len).unwrap_or(0);
            log::info!("synced {file}: {entries} entries");
            let message = if entries == 0 {
                "No changes to sync".to_string()
            } else {
                format!("Synced {entries} entries")
            };
            SyncStatus {
                file,
                ok: true,
                message,
                entries: Some(entries),
                at,
            }
        }
        Err(err) => {
            log::warn!("sync failed for {file}: {err}");
            SyncStatus {
                file,
                ok: false,
                message: err.to_string(),
                entries: None,
                at,
            }
        }
    };

    let _ = app.emit("sync-status", &status);
}

/// Best-effort canonicalization, falling back to the path as given.
fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Compare two paths, resolving symlinks/`.`/`..` when possible.
fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(x), Ok(y)) => x == y,
        _ => a == b,
    }
}
