//! Per-coordinate change tracking, so unchanged stockpiles aren't re-sent.
//!
//! Every entry in the extracted `PinnedMapToolTipsW` array carries a
//! `NormalizedMapCoords` (`{ x, y }`) that uniquely identifies a pinned
//! stockpile on the map, plus a `LastUpdated` timestamp the game bumps whenever
//! that stockpile's contents change. We remember the last `LastUpdated` we
//! successfully sent for each coordinate; on the next sync, entries whose
//! `LastUpdated` is unchanged are dropped from the payload's `data` list.
//!
//! The memory is keyed by `(filename, MapId, x, y)` — `MapId` is folded into the
//! key so identical normalized coordinates on different hex maps don't collide,
//! and the filename scopes it so multiple watched saves are tracked
//! independently. It is persisted with `tauri-plugin-store` so the dedup
//! survives app restarts.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

/// Store file name (relative to the app-config dir), separate from `config.json`.
pub const STORE_FILE: &str = "sync-state.json";
/// Key under which the [`SyncMemory`] blob is persisted.
pub const STORE_KEY: &str = "last_updated";

/// Entry field naming the stockpile's map (e.g. `EWorldConquestMapId::...`).
const MAP_ID_KEY: &str = "MapId";
/// Entry field holding the `{ x, y }` coordinates.
const COORDS_KEY: &str = "NormalizedMapCoords";
/// Entry field holding the per-stockpile update timestamp.
const LAST_UPDATED_KEY: &str = "LastUpdated";

/// Remembered `LastUpdated` values, grouped by watched filename and then by a
/// per-entry coordinate key.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncMemory {
    /// `filename -> (coord key -> last sent LastUpdated)`.
    files: HashMap<String, HashMap<String, Value>>,
}

/// The result of filtering an entry list: the entries to keep, plus the
/// `(coord key, LastUpdated)` pairs to remember once the send succeeds.
#[derive(Debug, Default)]
pub struct Selection {
    /// Entries that are new or whose `LastUpdated` changed — these get sent.
    pub kept: Vec<Value>,
    /// Updates to apply via [`SyncMemory::commit`] after a successful POST.
    pub updates: Vec<(String, Value)>,
}

impl SyncMemory {
    /// Split `items` into the entries worth sending and the memory updates that
    /// should follow a successful send.
    ///
    /// An entry is dropped only when we have a remembered `LastUpdated` for its
    /// coordinate and it is byte-for-byte equal to the entry's current value.
    /// Anything we can't key (missing coords) or can't compare (missing
    /// `LastUpdated`) is always kept, so we never silently lose data.
    pub fn filter(&self, filename: &str, items: Vec<Value>) -> Selection {
        let known = self.files.get(filename);
        let mut selection = Selection::default();

        for item in items {
            let key = coord_key(&item);
            let last_updated = item.get(LAST_UPDATED_KEY).cloned();

            let unchanged = match (&key, &last_updated) {
                (Some(key), Some(last_updated)) => {
                    known.and_then(|m| m.get(key)) == Some(last_updated)
                }
                _ => false,
            };
            if unchanged {
                continue;
            }

            if let (Some(key), Some(last_updated)) = (key, last_updated) {
                selection.updates.push((key, last_updated));
            }
            selection.kept.push(item);
        }

        selection
    }

    /// Record the `LastUpdated` values from a successful send so the matching
    /// entries are skipped next time unless they change again.
    pub fn commit(&mut self, filename: &str, updates: Vec<(String, Value)>) {
        if updates.is_empty() {
            return;
        }
        let entry = self.files.entry(filename.to_string()).or_default();
        for (key, last_updated) in updates {
            entry.insert(key, last_updated);
        }
    }
}

/// Build the stable dedup key for an entry, or `None` if it lacks coordinates.
///
/// `x`/`y` are rendered via their JSON form, which round-trips the exact `f64`
/// bits the save serializes — so an unchanged coordinate produces an identical
/// key across runs.
fn coord_key(item: &Value) -> Option<String> {
    let coords = item.get(COORDS_KEY)?;
    let x = coords.get("x")?;
    let y = coords.get("y")?;
    if x.is_null() || y.is_null() {
        return None;
    }
    let map_id = item.get(MAP_ID_KEY).and_then(Value::as_str).unwrap_or("");
    Some(format!("{map_id}|{x}|{y}"))
}

/// Load the persisted memory, or return an empty one if nothing is stored yet.
pub fn load(app: &AppHandle) -> SyncMemory {
    match app.store(STORE_FILE) {
        Ok(store) => store
            .get(STORE_KEY)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        Err(err) => {
            log::warn!("could not open sync-state store, starting empty: {err}");
            SyncMemory::default()
        }
    }
}

/// Persist the memory to disk.
pub fn save(app: &AppHandle, memory: &SyncMemory) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let value = serde_json::to_value(memory).map_err(|e| e.to_string())?;
    store.set(STORE_KEY, value);
    store.save().map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn entry(map_id: &str, x: f64, y: f64, last_updated: &str) -> Value {
        json!({
            "MapId": map_id,
            "NormalizedMapCoords": { "x": x, "y": y },
            "LastUpdated": last_updated,
            "RecentMapItemDetails": {}
        })
    }

    #[test]
    fn first_sync_keeps_everything() {
        let memory = SyncMemory::default();
        let items = vec![
            entry("MapId::A", 0.5, 0.25, "2026-05-17T20:23:45.875Z"),
            entry("MapId::A", 0.1, 0.9, "2026-05-17T20:23:45.875Z"),
        ];
        let selection = memory.filter("save.sav", items);
        assert_eq!(selection.kept.len(), 2);
        assert_eq!(selection.updates.len(), 2);
    }

    #[test]
    fn unchanged_entries_are_dropped_after_commit() {
        let mut memory = SyncMemory::default();
        let items = vec![entry("MapId::A", 0.5, 0.25, "2026-05-17T20:23:45.875Z")];

        let first = memory.filter("save.sav", items.clone());
        memory.commit("save.sav", first.updates);

        // Same LastUpdated on the next sync -> nothing to send.
        let second = memory.filter("save.sav", items);
        assert!(second.kept.is_empty());
        assert!(second.updates.is_empty());
    }

    #[test]
    fn changed_last_updated_is_resent() {
        let mut memory = SyncMemory::default();
        let first = memory.filter(
            "save.sav",
            vec![entry("MapId::A", 0.5, 0.25, "2026-05-17T20:23:45.875Z")],
        );
        memory.commit("save.sav", first.updates);

        let second = memory.filter(
            "save.sav",
            vec![entry("MapId::A", 0.5, 0.25, "2026-05-18T08:00:00.000Z")],
        );
        assert_eq!(second.kept.len(), 1);
        assert_eq!(second.updates.len(), 1);
    }

    #[test]
    fn same_coords_on_different_maps_do_not_collide() {
        let mut memory = SyncMemory::default();
        let first = memory.filter(
            "save.sav",
            vec![entry("MapId::A", 0.5, 0.25, "2026-05-17T20:23:45.875Z")],
        );
        memory.commit("save.sav", first.updates);

        // Identical coordinates, different map -> treated as a new stockpile.
        let other_map = memory.filter(
            "save.sav",
            vec![entry("MapId::B", 0.5, 0.25, "2026-05-17T20:23:45.875Z")],
        );
        assert_eq!(other_map.kept.len(), 1);
    }

    #[test]
    fn memory_is_scoped_per_file() {
        let mut memory = SyncMemory::default();
        let first = memory.filter(
            "a.sav",
            vec![entry("MapId::A", 0.5, 0.25, "2026-05-17T20:23:45.875Z")],
        );
        memory.commit("a.sav", first.updates);

        // The same entry from a different save file is still new.
        let other_file = memory.filter(
            "b.sav",
            vec![entry("MapId::A", 0.5, 0.25, "2026-05-17T20:23:45.875Z")],
        );
        assert_eq!(other_file.kept.len(), 1);
    }

    #[test]
    fn entries_without_last_updated_are_always_kept() {
        let mut memory = SyncMemory::default();
        let item = json!({
            "MapId": "MapId::A",
            "NormalizedMapCoords": { "x": 0.5, "y": 0.25 }
        });
        let first = memory.filter("save.sav", vec![item.clone()]);
        assert_eq!(first.kept.len(), 1);
        // Nothing to remember without a LastUpdated, so it can't be deduped.
        assert!(first.updates.is_empty());
        memory.commit("save.sav", first.updates);

        let second = memory.filter("save.sav", vec![item]);
        assert_eq!(second.kept.len(), 1);
    }
}
