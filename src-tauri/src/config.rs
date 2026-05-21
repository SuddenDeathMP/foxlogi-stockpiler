//! Persistent application configuration.
//!
//! Stored via `tauri-plugin-store` in the OS app-config directory
//! (e.g. `~/Library/Application Support/foxlogi-stockpiler/config.json`).

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_store::StoreExt;

/// Store file name (relative to the app-config dir).
pub const STORE_FILE: &str = "config.json";
/// Key under which the whole [`Config`] blob is persisted.
pub const STORE_KEY: &str = "config";

/// User-facing settings: the API key and the set of watched files.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Bearer token sent in the `Authorization` header.
    pub api_key: String,
    /// Absolute paths of the `.sav` files being watched.
    pub files: Vec<String>,
    /// Optional override of the server base URL. When `None`, the built-in
    /// [`crate::sync::DEFAULT_SERVER_BASE`] is used.
    pub server_url: Option<String>,
}

/// Load the persisted config, or return defaults if nothing is stored yet.
pub fn load(app: &AppHandle) -> Config {
    match app.store(STORE_FILE) {
        Ok(store) => store
            .get(STORE_KEY)
            .and_then(|value| serde_json::from_value(value).ok())
            .unwrap_or_default(),
        Err(err) => {
            log::warn!("could not open config store, using defaults: {err}");
            Config::default()
        }
    }
}

/// Persist the config to disk.
pub fn save(app: &AppHandle, config: &Config) -> Result<(), String> {
    let store = app.store(STORE_FILE).map_err(|e| e.to_string())?;
    let value = serde_json::to_value(config).map_err(|e| e.to_string())?;
    store.set(STORE_KEY, value);
    store.save().map_err(|e| e.to_string())
}
