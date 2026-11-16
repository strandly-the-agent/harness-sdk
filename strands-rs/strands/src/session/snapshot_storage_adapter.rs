//! Snapshot persistence over a [`Storage`] (TS `session/snapshot-storage-adapter.ts`).

use std::fmt;
use std::sync::Arc;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use super::validation::{scope_str, validate_identifier, validate_uuid_v7};
use crate::error::{BoxError, Error, Result};
use crate::storage::Storage;
use crate::types::json::{JsonValue, from_json_str, to_js_string};
use crate::types::snapshot::{Scope, Snapshot, iso_timestamp};

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "TS SessionManager never writes the manifest; the conformance tests do"
    )
)]
const SCHEMA_VERSION: &str = "1.0";
const DELETE_BATCH_SIZE: usize = 100;

/// Where a snapshot lives (TS `SnapshotLocation`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotLocation {
    pub(crate) session_id: String,
    pub(crate) scope: Scope,
    pub(crate) scope_id: String,
}

/// Per-scope snapshot metadata (TS `SnapshotManifest`).
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "TS SessionManager never writes the manifest; the conformance tests do"
    )
)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SnapshotManifest {
    pub(crate) schema_version: String,
    pub(crate) updated_at: String,
}

/// The TS `SessionError` (not exported at the pin): a message plus the underlying cause.
#[derive(Debug)]
struct SessionError {
    message: String,
    source: BoxError,
}

impl fmt::Display for SessionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for SessionError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.source)
    }
}

fn session_error(message: String, source: impl Into<BoxError>) -> Error {
    Error::other(SessionError {
        message,
        source: source.into(),
    })
}

/// Stores snapshots under `<sessionId>/scopes/<scope>/<scopeId>/snapshots/` (TS
/// `SnapshotStorageAdapter`).
#[derive(Clone)]
pub(crate) struct SnapshotStorageAdapter {
    storage: Arc<dyn Storage>,
}

impl SnapshotStorageAdapter {
    pub(crate) fn new(storage: Arc<dyn Storage>) -> Self {
        Self { storage }
    }

    /// Writes `snapshot_latest.json`, or the immutable `snapshot_<id>.json` when not latest.
    pub(crate) async fn save_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: &str,
        is_latest: bool,
        snapshot: &Snapshot,
    ) -> Result<()> {
        let key = if is_latest {
            latest_key(location)?
        } else {
            history_key(location, snapshot_id)?
        };
        self.write_json(&key, snapshot).await
    }

    /// The latest snapshot, or the immutable one with `snapshot_id`.
    pub(crate) async fn load_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: Option<&str>,
    ) -> Result<Option<Snapshot>> {
        let key = match snapshot_id {
            None => latest_key(location)?,
            Some(id) => history_key(location, id)?,
        };
        self.read_json(&key).await
    }

    /// Immutable snapshot ids in ascending order, after `start_after` and at most `limit`.
    pub(crate) async fn list_snapshot_ids(
        &self,
        location: &SnapshotLocation,
        limit: Option<usize>,
        start_after: Option<&str>,
    ) -> Result<Vec<String>> {
        if limit == Some(0) {
            return Ok(Vec::new());
        }
        let start_after = start_after.filter(|id| !id.is_empty());
        if let Some(id) = start_after {
            validate_uuid_v7(id)?;
        }
        let keys = self.storage.list(&history_prefix(location)?).await?;
        let mut ids: Vec<String> = keys.iter().filter_map(|key| snapshot_id_of(key)).collect();
        crate::storage::sort_keys(&mut ids);
        if let Some(after) = start_after {
            ids.retain(|id| id.encode_utf16().gt(after.encode_utf16()));
        }
        if let Some(limit) = limit {
            ids.truncate(limit);
        }
        Ok(ids)
    }

    /// Deletes every key under `<sessionId>/`, 100 at a time.
    pub(crate) async fn delete_session(&self, session_id: &str) -> Result<()> {
        validate_identifier(session_id)?;
        let keys = self.storage.list(&format!("{session_id}/")).await?;
        for batch in keys.chunks(DELETE_BATCH_SIZE) {
            let deletes = futures::future::join_all(batch.iter().map(|key| self.storage.delete(key))).await;
            deletes.into_iter().collect::<Result<Vec<()>>>()?;
        }
        Ok(())
    }

    /// The saved manifest, or a fresh one stamped now.
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "TS SessionManager never writes the manifest; the conformance tests do"
        )
    )]
    pub(crate) async fn load_manifest(&self, location: &SnapshotLocation) -> Result<SnapshotManifest> {
        let manifest = self.read_json(&manifest_key(location)?).await?;
        Ok(manifest.unwrap_or_else(|| SnapshotManifest {
            schema_version: SCHEMA_VERSION.to_owned(),
            updated_at: iso_timestamp(jiff::Timestamp::now()),
        }))
    }

    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "TS SessionManager never writes the manifest; the conformance tests do"
        )
    )]
    pub(crate) async fn save_manifest(
        &self,
        location: &SnapshotLocation,
        manifest: &SnapshotManifest,
    ) -> Result<()> {
        self.write_json(&manifest_key(location)?, manifest).await
    }

    async fn write_json<T: Serialize>(&self, key: &str, data: &T) -> Result<()> {
        let failed = |error: BoxError| session_error(format!("Failed to write '{key}' to storage"), error);
        let text = to_js_string(data).map_err(|error| failed(error.into()))?;
        self.storage
            .write(key, text.as_bytes())
            .await
            .map_err(|error| failed(error.into()))
    }

    async fn read_json<T: DeserializeOwned>(&self, key: &str) -> Result<Option<T>> {
        let bytes = self
            .storage
            .read(key)
            .await
            .map_err(|error| session_error(format!("Failed to read '{key}' from storage"), error))?;
        let Some(bytes) = bytes else { return Ok(None) };
        let corrupted = |error: serde_json::Error| session_error(format!("Corrupted JSON at '{key}'"), error);
        let value: JsonValue = from_json_str(&String::from_utf8_lossy(&bytes)).map_err(corrupted)?;
        serde_json::from_value(value).map(Some).map_err(corrupted)
    }
}

/// The id in a `snapshot_<id>.json` key (TS `/snapshot_([\w-]+)\.json$/`).
fn snapshot_id_of(key: &str) -> Option<String> {
    let stem = key.strip_suffix(".json")?;
    let word = |c: char| c.is_ascii_alphanumeric() || c == '_' || c == '-';
    stem.match_indices("snapshot_")
        .filter_map(|(start, marker)| stem.get(start + marker.len()..))
        .find(|id| !id.is_empty() && id.chars().all(word))
        .map(str::to_owned)
}

fn scope_prefix(location: &SnapshotLocation) -> Result<String> {
    validate_identifier(&location.session_id)?;
    validate_identifier(&location.scope_id)?;
    Ok(format!(
        "{}/scopes/{}/{}/snapshots",
        location.session_id,
        scope_str(location.scope),
        location.scope_id
    ))
}

fn latest_key(location: &SnapshotLocation) -> Result<String> {
    Ok(format!("{}/snapshot_latest.json", scope_prefix(location)?))
}

fn history_key(location: &SnapshotLocation, snapshot_id: &str) -> Result<String> {
    validate_identifier(snapshot_id)?;
    Ok(format!(
        "{}/immutable_history/snapshot_{snapshot_id}.json",
        scope_prefix(location)?
    ))
}

fn history_prefix(location: &SnapshotLocation) -> Result<String> {
    Ok(format!("{}/immutable_history/", scope_prefix(location)?))
}

#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "TS SessionManager never writes the manifest; the conformance tests do"
    )
)]
fn manifest_key(location: &SnapshotLocation) -> Result<String> {
    Ok(format!("{}/manifest.json", scope_prefix(location)?))
}

#[cfg(test)]
mod tests;
