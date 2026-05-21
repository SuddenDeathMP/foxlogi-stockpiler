//! Foxlogi Stockpiler — watches Foxhole `.sav` files and syncs extracted
//! stockpile data to a remote server.
//!
//! The whole pipeline (watch → parse → extract → POST) lives here in Rust; the
//! frontend is a thin API-key field plus a file picker. See the per-module docs
//! for details.

pub mod config;
pub mod dedup;
pub mod extract;
pub mod pipeline;
pub mod state;
pub mod sync;

use std::path::Path;

use tauri::{AppHandle, Manager, State};

use config::Config;
use state::AppState;

/// Return the current persisted config.
#[tauri::command]
fn get_config(state: State<'_, AppState>) -> Config {
    state.config()
}

/// Set the API key.
#[tauri::command]
fn set_api_key(state: State<'_, AppState>, api_key: String) -> Result<Config, String> {
    state.update_config(move |c| c.api_key = api_key)
}

/// Override (or clear, with an empty/null value) the server base URL.
#[tauri::command]
fn set_server_url(
    state: State<'_, AppState>,
    server_url: Option<String>,
) -> Result<Config, String> {
    state.update_config(move |c| {
        c.server_url = server_url.filter(|s| !s.trim().is_empty());
    })
}

/// The only `.sav` files Foxlogi watches are Foxhole map saves, which always
/// end in this suffix. The native picker can only filter by extension, so we
/// enforce the full pattern here after the user selects.
const MAP_DATA_SUFFIX: &str = "_MapData.sav";

/// Result of [`add_files`]: the updated config plus how many of the user's
/// picks were ignored because they didn't match `*_MapData.sav`.
#[derive(serde::Serialize)]
struct AddFilesResult {
    config: Config,
    skipped: usize,
}

/// Open a native file picker and add the chosen `*_MapData.sav` files to the
/// watch list. Non-matching `.sav` files the user selects are ignored and
/// counted in `skipped`. The config is unchanged if the user cancels.
#[tauri::command]
async fn add_files(app: AppHandle, state: State<'_, AppState>) -> Result<AddFilesResult, String> {
    let dialog_app = app.clone();
    // The native dialog must run off the command's async worker; `blocking_*`
    // internally dispatches to the main thread and waits for the user.
    let picked = tauri::async_runtime::spawn_blocking(move || {
        use tauri_plugin_dialog::DialogExt;
        dialog_app
            .dialog()
            .file()
            .add_filter("Foxhole map save", &["sav"])
            .set_title("Select *_MapData.sav files to watch")
            .blocking_pick_files()
    })
    .await
    .map_err(|e| e.to_string())?;

    let Some(picked) = picked else {
        // User cancelled — nothing picked, nothing skipped.
        return Ok(AddFilesResult {
            config: state.config(),
            skipped: 0,
        });
    };

    let all: Vec<String> = picked
        .into_iter()
        .filter_map(|fp| fp.into_path().ok())
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    let total = all.len();

    let selected: Vec<String> = all
        .into_iter()
        .filter(|p| {
            Path::new(p)
                .file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.ends_with(MAP_DATA_SUFFIX))
        })
        .collect();
    let skipped = total - selected.len();

    let config = state.update_config(move |c| {
        for path in selected {
            if !c.files.contains(&path) {
                c.files.push(path);
            }
        }
    })?;

    Ok(AddFilesResult { config, skipped })
}

/// Remove a file from the watch list.
#[tauri::command]
fn remove_file(state: State<'_, AppState>, path: String) -> Result<Config, String> {
    state.update_config(move |c| c.files.retain(|f| f != &path))
}

/// Manually run the pipeline for one watched file right now.
#[tauri::command]
fn sync_now(state: State<'_, AppState>, path: String) -> Result<usize, String> {
    state.sync_file(Path::new(&path))
}

/// The full server endpoint the app will POST to (for display in the UI).
#[tauri::command]
fn endpoint_url(state: State<'_, AppState>) -> String {
    let config = state.config();
    sync::endpoint(
        config
            .server_url
            .as_deref()
            .unwrap_or(sync::DEFAULT_SERVER_BASE),
    )
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .setup(|app| {
            let state = AppState::new(app.handle().clone());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_config,
            set_api_key,
            set_server_url,
            add_files,
            remove_file,
            sync_now,
            endpoint_url
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
