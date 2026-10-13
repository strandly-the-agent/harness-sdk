//! The snapshot storage interface (`strands-ts/src/session/storage.ts`).

use async_trait::async_trait;

use super::types::{Scope, Snapshot, SnapshotManifest};
use crate::error::Result;

/// Identifies the location of a snapshot within the storage hierarchy.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnapshotLocation {
    /// Session identifier.
    pub session_id: String,
    /// Scope of the snapshot.
    pub scope: Scope,
    /// Scope-specific identifier (agent id or orchestrator id).
    pub scope_id: String,
}

impl SnapshotLocation {
    /// Location of an agent-scoped snapshot.
    pub fn agent(session_id: impl Into<String>, agent_id: impl Into<String>) -> Self {
        Self {
            session_id: session_id.into(),
            scope: Scope::Agent,
            scope_id: agent_id.into(),
        }
    }
}

/// Snapshot persistence backend. Implementations provide filesystem, S3, … storage.
///
/// `SessionStorage` in TS is just `{ snapshot }`; the wrapper is flattened away here.
#[async_trait]
pub trait SnapshotStorage: Send + Sync {
    /// Persists a snapshot. `snapshot_id` is ignored when `is_latest` is set.
    async fn save_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: &str,
        is_latest: bool,
        snapshot: &Snapshot,
    ) -> Result<()>;

    /// Loads a snapshot, or the latest one when `snapshot_id` is `None`. Absent is `Ok(None)`.
    async fn load_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: Option<&str>,
    ) -> Result<Option<Snapshot>>;

    /// Immutable-history snapshot ids, sorted ascending. `start_after` is an exclusive cursor.
    async fn list_snapshot_ids(
        &self,
        location: &SnapshotLocation,
        limit: Option<usize>,
        start_after: Option<&str>,
    ) -> Result<Vec<String>>;

    /// Deletes every snapshot and directory belonging to the session id.
    async fn delete_session(&self, session_id: &str) -> Result<()>;

    /// Loads the snapshot manifest, synthesizing a default when none exists.
    async fn load_manifest(&self, location: &SnapshotLocation) -> Result<SnapshotManifest>;

    /// Persists the snapshot manifest.
    async fn save_manifest(&self, location: &SnapshotLocation, manifest: &SnapshotManifest) -> Result<()>;
}
