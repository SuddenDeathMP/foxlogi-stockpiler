//! HTTP sync: POST extracted data to the configured server.

use serde::Serialize;
use serde_json::Value;

/// Default server base URL. Can be overridden at runtime via the settings file
/// ([`crate::config::Config::server_url`]).
///
/// Dev builds (`tauri dev` / any `cargo` debug build) point at a local server
/// over plain HTTP; release builds use the production HTTPS endpoint.
pub const DEFAULT_SERVER_BASE: &str = if cfg!(debug_assertions) {
    "http://127.0.0.1:3000"
} else {
    "https://foxlogi.com"
};


/// it matches the contract documented in the README.
pub const STOCKPILE_PATH: &str = "/api/stockpile/bulk-update/";

/// The JSON body POSTed to the server.
#[derive(Debug, Clone, Serialize)]
pub struct UpdatePayload {
    /// Name of the `.sav` file the data came from.
    pub filename: String,
    /// File modification time, ISO-8601 / RFC-3339.
    pub modified_at: String,
    /// The extracted, cleaned `PinnedMapToolTipsW` array.
    pub data: Value,
}

/// Errors that can occur while syncing.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("API key is not set")]
    MissingApiKey,
    #[error("HTTP request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("server returned {status}: {body}")]
    Status { status: u16, body: String },
}

/// Build the full endpoint URL from a base URL.
pub fn endpoint(base_url: &str) -> String {
    format!("{}{}", base_url.trim_end_matches('/'), STOCKPILE_PATH)
}

/// POST a payload to the server with the API key as a bearer token.
pub fn post_update(
    client: &reqwest::blocking::Client,
    base_url: &str,
    api_key: &str,
    payload: &UpdatePayload,
) -> Result<(), SyncError> {
    if api_key.trim().is_empty() {
        return Err(SyncError::MissingApiKey);
    }

    let response = client
        .post(endpoint(base_url))
        .bearer_auth(api_key)
        .json(payload)
        .send()?;

    let status = response.status();
    if !status.is_success() {
        let body = response.text().unwrap_or_default();
        return Err(SyncError::Status {
            status: status.as_u16(),
            body,
        });
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_joins_without_double_slash() {
        assert_eq!(
            endpoint("https://foxlogi.com/"),
            "https://foxlogi.com/api/stockpile/bulk-update/"
        );
        assert_eq!(
            endpoint("https://foxlogi.com"),
            "https://foxlogi.com/api/stockpile/bulk-update/"
        );
    }

    #[test]
    fn empty_api_key_is_rejected() {
        let client = reqwest::blocking::Client::new();
        let payload = UpdatePayload {
            filename: "x.sav".into(),
            modified_at: "2026-01-01T00:00:00Z".into(),
            data: Value::Array(vec![]),
        };
        let err = post_update(&client, DEFAULT_SERVER_BASE, "  ", &payload).unwrap_err();
        assert!(matches!(err, SyncError::MissingApiKey));
    }
}
