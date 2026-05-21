//! The end-to-end pipeline for a single file: read -> parse -> extract -> POST.

use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};

use crate::{extract, sync};

/// Errors from running the pipeline on one file.
#[derive(Debug, thiserror::Error)]
pub enum PipelineError {
    #[error("could not read file: {0}")]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Extract(#[from] extract::ExtractError),
    #[error(transparent)]
    Sync(#[from] sync::SyncError),
}

/// File modification time as an ISO-8601 / RFC-3339 UTC string.
pub fn modified_at_iso(path: &Path) -> std::io::Result<String> {
    let modified = std::fs::metadata(path)?.modified()?;
    Ok(DateTime::<Utc>::from(modified).to_rfc3339_opts(SecondsFormat::Secs, true))
}

/// Read a `.sav`, extract its data, and build the API payload (no network).
pub fn build_payload(path: &Path) -> Result<sync::UpdatePayload, PipelineError> {
    let bytes = std::fs::read(path)?;
    let data = extract::extract_pinned_tooltips(&bytes)?;
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("unknown.sav")
        .to_string();
    let modified_at = modified_at_iso(path)?;
    Ok(sync::UpdatePayload {
        filename,
        modified_at,
        data,
    })
}
