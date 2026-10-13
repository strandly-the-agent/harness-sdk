//! In-memory [`SnapshotStorage`] for tests (`MockSnapshotStorage` in the TS fixtures).

use std::collections::HashMap;

use async_trait::async_trait;
use parking_lot::Mutex;

use super::storage::{SnapshotLocation, SnapshotStorage};
use super::types::{now_iso8601, SnapshotManifest, SNAPSHOT_SCHEMA_VERSION};
use super::Snapshot;
use crate::error::{Error, Result};

#[derive(Default)]
pub(crate) struct MockSnapshotStorage {
    snapshots: Mutex<HashMap<String, Snapshot>>,
    manifests: Mutex<HashMap<String, SnapshotManifest>>,
    pub(crate) should_throw_errors: Mutex<bool>,
    pub(crate) save_count: Mutex<usize>,
}

fn scope_key(location: &SnapshotLocation) -> String {
    format!(
        "{}/{}/{}",
        location.session_id,
        location.scope.as_str(),
        location.scope_id
    )
}

impl MockSnapshotStorage {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    fn check(&self) -> Result<()> {
        if *self.should_throw_errors.lock() {
            return Err(Error::Session {
                message: "Mock storage error".to_owned(),
                source: None,
            });
        }
        Ok(())
    }

    pub(crate) fn saves(&self) -> usize {
        *self.save_count.lock()
    }
}

#[async_trait]
impl SnapshotStorage for MockSnapshotStorage {
    async fn save_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: &str,
        is_latest: bool,
        snapshot: &Snapshot,
    ) -> Result<()> {
        self.check()?;
        let id = if is_latest { "latest" } else { snapshot_id };
        self.snapshots
            .lock()
            .insert(format!("{}/{id}", scope_key(location)), snapshot.clone());
        *self.save_count.lock() += 1;
        Ok(())
    }

    async fn load_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: Option<&str>,
    ) -> Result<Option<Snapshot>> {
        self.check()?;
        let id = snapshot_id.unwrap_or("latest");
        Ok(self
            .snapshots
            .lock()
            .get(&format!("{}/{id}", scope_key(location)))
            .cloned())
    }

    async fn list_snapshot_ids(
        &self,
        location: &SnapshotLocation,
        limit: Option<usize>,
        start_after: Option<&str>,
    ) -> Result<Vec<String>> {
        self.check()?;
        if limit == Some(0) {
            return Ok(Vec::new());
        }
        let prefix = format!("{}/", scope_key(location));
        let mut ids: Vec<String> = self
            .snapshots
            .lock()
            .keys()
            .filter_map(|key| key.strip_prefix(&prefix).map(str::to_owned))
            .filter(|id| id != "latest")
            .collect();
        ids.sort();
        if let Some(cursor) = start_after {
            ids.retain(|id| id.as_str() > cursor);
        }
        if let Some(limit) = limit {
            ids.truncate(limit);
        }
        Ok(ids)
    }

    async fn delete_session(&self, session_id: &str) -> Result<()> {
        self.check()?;
        let prefix = format!("{session_id}/");
        self.snapshots.lock().retain(|k, _| !k.starts_with(&prefix));
        self.manifests.lock().retain(|k, _| !k.starts_with(&prefix));
        Ok(())
    }

    async fn load_manifest(&self, location: &SnapshotLocation) -> Result<SnapshotManifest> {
        self.check()?;
        Ok(self
            .manifests
            .lock()
            .get(&scope_key(location))
            .cloned()
            .unwrap_or_else(|| SnapshotManifest {
                schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
                updated_at: now_iso8601(),
            }))
    }

    async fn save_manifest(&self, location: &SnapshotLocation, manifest: &SnapshotManifest) -> Result<()> {
        self.check()?;
        self.manifests
            .lock()
            .insert(scope_key(location), manifest.clone());
        Ok(())
    }
}
