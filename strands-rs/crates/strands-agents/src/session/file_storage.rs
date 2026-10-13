//! Filesystem snapshot storage (`strands-ts/src/session/file-storage.ts`).

use std::path::{Path, PathBuf};

use async_trait::async_trait;

use super::storage::{SnapshotLocation, SnapshotStorage};
use super::types::{now_iso8601, Snapshot, SnapshotManifest, SNAPSHOT_SCHEMA_VERSION};
use super::validation::{validate_identifier, validate_uuid_v7};
use crate::error::{BoxError, Error, Result};

const MANIFEST: &str = "manifest.json";
const SNAPSHOT_LATEST: &str = "snapshot_latest.json";
const IMMUTABLE_HISTORY: &str = "immutable_history";

/// File-based [`SnapshotStorage`]: persists snapshots under a base directory.
///
/// Layout: `<base_dir>/<session_id>/scopes/<scope>/<scope_id>/snapshots/` containing
/// `snapshot_latest.json`, `manifest.json` and `immutable_history/snapshot_<uuid7>.json`.
#[derive(Clone, Debug)]
pub struct FileStorage {
    base_dir: PathBuf,
}

impl FileStorage {
    /// Storage rooted at `base_dir` (an absolute path).
    pub fn new(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: base_dir.into(),
        }
    }

    fn get_path(&self, location: &SnapshotLocation, filename: &str) -> Result<PathBuf> {
        validate_identifier(&location.session_id)?;
        validate_identifier(&location.scope_id)?;
        Ok(self
            .base_dir
            .join(&location.session_id)
            .join("scopes")
            .join(location.scope.as_str())
            .join(&location.scope_id)
            .join("snapshots")
            .join(filename))
    }

    fn get_session_dir(&self, session_id: &str) -> Result<PathBuf> {
        validate_identifier(session_id)?;
        Ok(self.base_dir.join(session_id))
    }

    fn latest_snapshot_path(&self, location: &SnapshotLocation) -> Result<PathBuf> {
        self.get_path(location, SNAPSHOT_LATEST)
    }

    fn history_snapshot_path(&self, location: &SnapshotLocation, snapshot_id: &str) -> Result<PathBuf> {
        validate_identifier(snapshot_id)?;
        let resolved = self.get_path(
            location,
            &format!("{IMMUTABLE_HISTORY}/snapshot_{snapshot_id}.json"),
        )?;
        if !resolved.starts_with(&self.base_dir) {
            return Err(Error::Session {
                message: format!("Invalid snapshotId '{snapshot_id}': resolves outside storage directory"),
                source: None,
            });
        }
        Ok(resolved)
    }

    async fn write_json<T: serde::Serialize>(&self, path: &Path, data: &T) -> Result<()> {
        write_json_atomically(path, data)
            .await
            .map_err(|err| session_error(format!("Failed to write file {}", path.display()), err))
    }

    async fn read_json<T: serde::de::DeserializeOwned>(&self, path: &Path) -> Result<Option<T>> {
        let bytes = match tokio::fs::read(path).await {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(session_error(
                    format!("File system error reading {}", path.display()),
                    err,
                ))
            }
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|err| session_error(format!("Invalid JSON in file {}", path.display()), err))
    }
}

async fn write_json_atomically<T: serde::Serialize>(
    path: &Path,
    data: &T,
) -> std::result::Result<(), BoxError> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let mut tmp_path = path.as_os_str().to_owned();
    tmp_path.push(".tmp");
    let tmp_path = PathBuf::from(tmp_path);
    tokio::fs::write(&tmp_path, serde_json::to_vec_pretty(data)?).await?;
    tokio::fs::rename(&tmp_path, path).await?;
    Ok(())
}

fn session_error(message: String, source: impl Into<BoxError>) -> Error {
    Error::Session {
        message,
        source: Some(source.into()),
    }
}

/// `snapshot_([\w-]+)\.json$` applied to a file name.
fn snapshot_id_from_file_name(name: &str) -> Option<&str> {
    let rest = name.strip_suffix(".json")?;
    rest.match_indices("snapshot_")
        .map(|(i, m)| &rest[i + m.len()..])
        .find(|id| {
            !id.is_empty()
                && id
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        })
}

#[async_trait]
impl SnapshotStorage for FileStorage {
    async fn save_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: &str,
        is_latest: bool,
        snapshot: &Snapshot,
    ) -> Result<()> {
        let path = if is_latest {
            self.latest_snapshot_path(location)?
        } else {
            self.history_snapshot_path(location, snapshot_id)?
        };
        self.write_json(&path, snapshot).await
    }

    async fn load_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: Option<&str>,
    ) -> Result<Option<Snapshot>> {
        let path = match snapshot_id {
            None => self.latest_snapshot_path(location)?,
            Some(id) => self.history_snapshot_path(location, id)?,
        };
        self.read_json(&path).await
    }

    async fn list_snapshot_ids(
        &self,
        location: &SnapshotLocation,
        limit: Option<usize>,
        start_after: Option<&str>,
    ) -> Result<Vec<String>> {
        if limit == Some(0) {
            return Ok(Vec::new());
        }
        if let Some(cursor) = start_after {
            validate_uuid_v7(cursor)?;
        }
        let dir_path = self.get_path(location, IMMUTABLE_HISTORY)?;
        let mut dir = match tokio::fs::read_dir(&dir_path).await {
            Ok(dir) => dir,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(err) => {
                return Err(session_error(
                    format!("Failed to list snapshots for session {}", location.session_id),
                    err,
                ))
            }
        };
        let mut ids = Vec::new();
        loop {
            let entry = dir.next_entry().await.map_err(|err| {
                session_error(
                    format!("Failed to list snapshots for session {}", location.session_id),
                    err,
                )
            })?;
            let Some(entry) = entry else { break };
            let name = entry.file_name();
            if let Some(id) = name.to_str().and_then(snapshot_id_from_file_name) {
                ids.push(id.to_owned());
            }
        }
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
        let session_dir = self.get_session_dir(session_id)?;
        match tokio::fs::remove_dir_all(&session_dir).await {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(session_error(
                format!("Failed to delete session {session_id}"),
                err,
            )),
        }
    }

    async fn load_manifest(&self, location: &SnapshotLocation) -> Result<SnapshotManifest> {
        let path = self.get_path(location, MANIFEST)?;
        Ok(self.read_json(&path).await?.unwrap_or_else(|| SnapshotManifest {
            schema_version: SNAPSHOT_SCHEMA_VERSION.to_owned(),
            updated_at: now_iso8601(),
        }))
    }

    async fn save_manifest(&self, location: &SnapshotLocation, manifest: &SnapshotManifest) -> Result<()> {
        let path = self.get_path(location, MANIFEST)?;
        self.write_json(&path, manifest).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::session::types::Scope;
    use serde_json::{json, Map};
    use tempfile::TempDir;

    const SCOPE_ID: &str = "test-agent";
    const IDS: [&str; 3] = [
        "019c9bf1-14e5-7eef-96fb-cc07ae54210f",
        "019c9bf1-1d34-7eef-96fb-d1be20fd7bbd",
        "019c9bf1-24bb-7eef-96fb-ddcc943cd859",
    ];

    fn test_snapshot() -> Snapshot {
        Snapshot {
            scope: Scope::Agent,
            schema_version: "1.0".to_owned(),
            created_at: "2024-01-01T00:00:00.000Z".to_owned(),
            data: json!({
                "messages": [],
                "state": { "testKey": "testValue" },
                "systemPrompt": "You are a test assistant",
            })
            .as_object()
            .unwrap()
            .clone(),
            app_data: Map::new(),
        }
    }

    fn test_manifest() -> SnapshotManifest {
        SnapshotManifest {
            schema_version: "1.0".to_owned(),
            updated_at: "2024-01-01T00:00:00.000Z".to_owned(),
        }
    }

    fn storage() -> (TempDir, FileStorage) {
        let dir = tempfile::tempdir().unwrap();
        let storage = FileStorage::new(dir.path());
        (dir, storage)
    }

    fn location() -> SnapshotLocation {
        SnapshotLocation::agent("test-session", SCOPE_ID)
    }

    fn snapshots_dir(base: &Path, location: &SnapshotLocation) -> PathBuf {
        base.join(&location.session_id)
            .join("scopes")
            .join(location.scope.as_str())
            .join(&location.scope_id)
            .join("snapshots")
    }

    async fn read_snapshot(path: &Path) -> Snapshot {
        serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap()
    }

    #[tokio::test]
    async fn saves_snapshot_to_history_file() {
        let (dir, storage) = storage();
        let snapshot = test_snapshot();

        storage
            .save_snapshot(&location(), "1", false, &snapshot)
            .await
            .unwrap();

        let history_path = snapshots_dir(dir.path(), &location())
            .join("immutable_history")
            .join("snapshot_1.json");
        assert_eq!(read_snapshot(&history_path).await, snapshot);
    }

    #[tokio::test]
    async fn saves_snapshot_as_latest_when_is_latest_is_true() {
        let (dir, storage) = storage();
        let snapshot = test_snapshot();

        storage
            .save_snapshot(&location(), "1", true, &snapshot)
            .await
            .unwrap();

        let latest_path = snapshots_dir(dir.path(), &location()).join("snapshot_latest.json");
        assert_eq!(read_snapshot(&latest_path).await, snapshot);
    }

    #[tokio::test]
    async fn creates_directories_recursively() {
        let (dir, storage) = storage();
        let location = SnapshotLocation::agent("new-session", "new-agent");

        storage
            .save_snapshot(&location, "1", true, &test_snapshot())
            .await
            .unwrap();

        assert!(snapshots_dir(dir.path(), &location).is_dir());
    }

    #[tokio::test]
    async fn throws_session_error_when_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        tokio::fs::write(&blocker, "not a directory").await.unwrap();
        let storage = FileStorage::new(&blocker);

        let err = storage
            .save_snapshot(&location(), "1", false, &test_snapshot())
            .await
            .unwrap_err();

        assert_eq!(err.name(), "SessionError");
        assert!(err.to_string().starts_with("Failed to write file "), "{err}");
    }

    #[tokio::test]
    async fn rejects_a_scope_id_containing_path_traversal() {
        let (dir, storage) = storage();
        let location = SnapshotLocation::agent("test-session", "../../../../outside");

        let err = storage
            .save_snapshot(&location, "1", true, &test_snapshot())
            .await
            .unwrap_err();

        assert_eq!(
            err.to_string(),
            "Identifier '../../../../outside' can only contain lowercase letters, numbers, hyphens, and underscores"
        );
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
    }

    #[tokio::test]
    async fn saves_multi_agent_snapshot_to_correct_path() {
        let (dir, storage) = storage();
        let location = SnapshotLocation {
            session_id: "multi-session".to_owned(),
            scope: Scope::MultiAgent,
            scope_id: "graph-1".to_owned(),
        };
        let snapshot = Snapshot {
            scope: Scope::MultiAgent,
            ..test_snapshot()
        };

        storage
            .save_snapshot(&location, "1", true, &snapshot)
            .await
            .unwrap();

        let expected = dir
            .path()
            .join("multi-session")
            .join("scopes")
            .join("multiAgent")
            .join("graph-1")
            .join("snapshots")
            .join("snapshot_latest.json");
        assert_eq!(read_snapshot(&expected).await, snapshot);
    }

    #[tokio::test]
    async fn loads_latest_snapshot_when_snapshot_id_is_none() {
        let (_dir, storage) = storage();
        let snapshot = test_snapshot();
        storage
            .save_snapshot(&location(), "1", true, &snapshot)
            .await
            .unwrap();

        assert_eq!(
            storage.load_snapshot(&location(), None).await.unwrap(),
            Some(snapshot)
        );
    }

    #[tokio::test]
    async fn loads_specific_snapshot_by_id() {
        let (_dir, storage) = storage();
        let snapshot = test_snapshot();
        storage
            .save_snapshot(&location(), "5", false, &snapshot)
            .await
            .unwrap();

        assert_eq!(
            storage.load_snapshot(&location(), Some("5")).await.unwrap(),
            Some(snapshot)
        );
    }

    #[tokio::test]
    async fn returns_null_when_snapshot_file_does_not_exist() {
        let (_dir, storage) = storage();
        let location = SnapshotLocation::agent("nonexistent", SCOPE_ID);

        assert_eq!(storage.load_snapshot(&location, None).await.unwrap(), None);
    }

    #[tokio::test]
    async fn throws_session_error_when_snapshot_json_is_invalid() {
        let (dir, storage) = storage();
        let snapshots = snapshots_dir(dir.path(), &location());
        tokio::fs::create_dir_all(&snapshots).await.unwrap();
        tokio::fs::write(snapshots.join("snapshot_latest.json"), "invalid json")
            .await
            .unwrap();

        let err = storage.load_snapshot(&location(), None).await.unwrap_err();

        assert_eq!(err.name(), "SessionError");
        assert!(err.to_string().starts_with("Invalid JSON in file "), "{err}");
    }

    #[tokio::test]
    async fn throws_session_error_when_file_read_fails() {
        let (dir, storage) = storage();
        let snapshots = snapshots_dir(dir.path(), &location());
        tokio::fs::create_dir_all(snapshots.join("snapshot_latest.json"))
            .await
            .unwrap();

        let err = storage.load_snapshot(&location(), None).await.unwrap_err();

        assert_eq!(err.name(), "SessionError");
        assert!(err.to_string().starts_with("File system error reading "), "{err}");
    }

    async fn save_history(storage: &FileStorage, ids: &[&str]) {
        for id in ids {
            storage
                .save_snapshot(&location(), id, false, &test_snapshot())
                .await
                .unwrap();
        }
    }

    #[tokio::test]
    async fn returns_sorted_snapshot_ids() {
        let (_dir, storage) = storage();
        save_history(&storage, &[IDS[2], IDS[0], IDS[1]]).await;

        let result = storage.list_snapshot_ids(&location(), None, None).await.unwrap();

        assert_eq!(result, IDS);
    }

    #[tokio::test]
    async fn ignores_non_snapshot_files() {
        let (dir, storage) = storage();
        save_history(&storage, &[IDS[0]]).await;
        let history_dir = snapshots_dir(dir.path(), &location()).join("immutable_history");
        tokio::fs::write(history_dir.join("other-file.txt"), "not a snapshot")
            .await
            .unwrap();

        let result = storage.list_snapshot_ids(&location(), None, None).await.unwrap();

        assert_eq!(result, [IDS[0]]);
    }

    #[tokio::test]
    async fn filters_by_start_after_for_pagination() {
        let (_dir, storage) = storage();
        save_history(&storage, &IDS).await;

        let result = storage
            .list_snapshot_ids(&location(), None, Some(IDS[0]))
            .await
            .unwrap();

        assert_eq!(result, [IDS[1], IDS[2]]);
    }

    #[tokio::test]
    async fn limits_results_when_limit_is_provided() {
        let (_dir, storage) = storage();
        save_history(&storage, &IDS).await;

        let result = storage
            .list_snapshot_ids(&location(), Some(2), None)
            .await
            .unwrap();

        assert_eq!(result, [IDS[0], IDS[1]]);
    }

    #[tokio::test]
    async fn combines_start_after_and_limit() {
        let (_dir, storage) = storage();
        save_history(&storage, &IDS).await;

        let result = storage
            .list_snapshot_ids(&location(), Some(1), Some(IDS[0]))
            .await
            .unwrap();

        assert_eq!(result, [IDS[1]]);
    }

    #[tokio::test]
    async fn returns_empty_array_when_directory_does_not_exist() {
        let (_dir, storage) = storage();
        let location = SnapshotLocation::agent("nonexistent", SCOPE_ID);

        let result = storage.list_snapshot_ids(&location, None, None).await.unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn returns_empty_array_when_limit_is_zero() {
        let (_dir, storage) = storage();
        save_history(&storage, &IDS).await;

        let result = storage
            .list_snapshot_ids(&location(), Some(0), None)
            .await
            .unwrap();

        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn rejects_a_start_after_cursor_that_is_not_a_uuid_v7() {
        let (_dir, storage) = storage();

        let err = storage
            .list_snapshot_ids(&location(), None, Some("../evil"))
            .await
            .unwrap_err();

        assert_eq!(err.to_string(), "'../evil' is not a valid UUID v7 snapshot ID");
    }

    #[tokio::test]
    async fn throws_session_error_when_readdir_fails_with_non_enoent_error() {
        let (dir, storage) = storage();
        let snapshots = snapshots_dir(dir.path(), &location());
        tokio::fs::create_dir_all(&snapshots).await.unwrap();
        tokio::fs::write(snapshots.join("immutable_history"), "not a directory")
            .await
            .unwrap();

        let err = storage
            .list_snapshot_ids(&location(), None, None)
            .await
            .unwrap_err();

        assert_eq!(err.name(), "SessionError");
        assert_eq!(
            err.to_string(),
            "Failed to list snapshots for session test-session"
        );
    }

    #[tokio::test]
    async fn removes_the_entire_session_directory() {
        let (dir, storage) = storage();
        storage
            .save_snapshot(&location(), "1", true, &test_snapshot())
            .await
            .unwrap();

        storage.delete_session("test-session").await.unwrap();

        assert!(!dir.path().join("test-session").exists());
    }

    #[tokio::test]
    async fn no_ops_when_session_directory_does_not_exist() {
        let (_dir, storage) = storage();
        assert!(storage.delete_session("nonexistent-session").await.is_ok());
    }

    #[tokio::test]
    async fn throws_session_error_when_rm_fails() {
        let (dir, storage) = storage();
        tokio::fs::write(dir.path().join("test-session"), "not a directory")
            .await
            .unwrap();

        let err = storage.delete_session("test-session").await.unwrap_err();

        assert_eq!(err.name(), "SessionError");
        assert_eq!(err.to_string(), "Failed to delete session test-session");
    }

    #[tokio::test]
    async fn saves_manifest_to_correct_path() {
        let (dir, storage) = storage();
        let manifest = test_manifest();

        storage.save_manifest(&location(), &manifest).await.unwrap();

        let path = snapshots_dir(dir.path(), &location()).join("manifest.json");
        let written: SnapshotManifest =
            serde_json::from_slice(&tokio::fs::read(path).await.unwrap()).unwrap();
        assert_eq!(written, manifest);
    }

    #[tokio::test]
    async fn throws_session_error_when_manifest_write_fails() {
        let dir = tempfile::tempdir().unwrap();
        let blocker = dir.path().join("blocker");
        tokio::fs::write(&blocker, "not a directory").await.unwrap();
        let storage = FileStorage::new(&blocker);

        let err = storage
            .save_manifest(&location(), &test_manifest())
            .await
            .unwrap_err();

        assert_eq!(err.name(), "SessionError");
        assert!(err.to_string().starts_with("Failed to write file "), "{err}");
    }

    #[tokio::test]
    async fn loads_manifest_from_file() {
        let (_dir, storage) = storage();
        let manifest = test_manifest();
        storage.save_manifest(&location(), &manifest).await.unwrap();

        assert_eq!(storage.load_manifest(&location()).await.unwrap(), manifest);
    }

    #[tokio::test]
    async fn returns_default_manifest_when_manifest_file_does_not_exist() {
        let (_dir, storage) = storage();
        let location = SnapshotLocation::agent("nonexistent", SCOPE_ID);

        let manifest = storage.load_manifest(&location).await.unwrap();

        assert_eq!(manifest.schema_version, "1.0");
        assert!(manifest.updated_at.ends_with('Z'));
    }

    #[tokio::test]
    async fn throws_session_error_when_manifest_json_is_invalid() {
        let (dir, storage) = storage();
        let snapshots = snapshots_dir(dir.path(), &location());
        tokio::fs::create_dir_all(&snapshots).await.unwrap();
        tokio::fs::write(snapshots.join("manifest.json"), "invalid json")
            .await
            .unwrap();

        let err = storage.load_manifest(&location()).await.unwrap_err();

        assert_eq!(err.name(), "SessionError");
        assert!(err.to_string().starts_with("Invalid JSON in file "), "{err}");
    }

    #[test]
    fn snapshot_regex_matches_the_js_pattern() {
        assert_eq!(snapshot_id_from_file_name("snapshot_1.json"), Some("1"));
        assert_eq!(
            snapshot_id_from_file_name("snapshot_a_snapshot_b.json"),
            Some("a_snapshot_b")
        );
        assert_eq!(snapshot_id_from_file_name("x.snapshot_b.json"), Some("b"));
        assert_eq!(snapshot_id_from_file_name("other-file.txt"), None);
        assert_eq!(snapshot_id_from_file_name("snapshot_.json"), None);
        assert_eq!(snapshot_id_from_file_name("snapshot_1.json.tmp"), None);
    }

    #[tokio::test]
    async fn writes_pretty_json_without_a_trailing_newline() {
        let (dir, storage) = storage();
        storage
            .save_snapshot(&location(), "1", true, &test_snapshot())
            .await
            .unwrap();

        let path = snapshots_dir(dir.path(), &location()).join("snapshot_latest.json");
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        assert!(content.starts_with("{\n  \"scope\": \"agent\","), "{content}");
        assert!(content.ends_with('}'), "{content}");
        assert!(!path.with_extension("json.tmp").exists());
    }
}
