//! Local-filesystem [`Storage`] backend (`strands-ts/src/storage/local-file-storage.ts`).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;

use super::search::{KeywordSearchStrategy, SearchStrategy, SearchStrategyRef};
use super::{normalize_key, normalize_prefix, Storage, StorageRef, StorageSearchResult};
use crate::error::{Error, Result};

const TMP_MARKER: &str = ".__strands_tmp";

/// Keys are stored as files under a base directory, `/` segments mapping onto directories.
/// Writes are atomic (scratch sibling + rename).
pub struct LocalFileStorage {
    base_dir: String,
    search_strategy: Option<SearchStrategyRef>,
    namespaced: bool,
}

impl LocalFileStorage {
    /// Storage rooted at `base_dir` (slash runs collapsed, trailing `/` stripped).
    pub fn new(base_dir: impl AsRef<str>) -> Self {
        Self {
            base_dir: normalize_base_dir(base_dir.as_ref()),
            search_strategy: None,
            namespaced: false,
        }
    }

    /// Same storage with `strategy` used for [`Storage::search`] and indexed on every write.
    pub fn with_search_strategy(mut self, strategy: impl SearchStrategy + 'static) -> Self {
        self.search_strategy = Some(Arc::new(strategy));
        self
    }

    /// The resolved root directory.
    pub fn base_dir(&self) -> &str {
        &self.base_dir
    }

    /// A storage rooted at the `prefix` subdirectory, carrying the search strategy over.
    pub fn namespaced(&self, prefix: &str) -> Self {
        let normalized = normalize_prefix(prefix).unwrap_or_default();
        let normalized = normalized.trim_end_matches('/');
        let base_dir = if normalized.is_empty() {
            self.base_dir.clone()
        } else {
            format!("{}/{}", self.base_dir, normalized)
        };
        Self {
            base_dir,
            search_strategy: self.search_strategy.clone(),
            namespaced: true,
        }
    }

    fn path_for(&self, key: &str) -> PathBuf {
        PathBuf::from(format!("{}/{}", self.base_dir, key))
    }

    async fn list_keys(&self, dir: PathBuf, key_prefix: String) -> Result<Vec<String>> {
        let mut found = Vec::new();
        let mut stack = vec![(dir, key_prefix)];
        while let Some((walk_dir, walk_prefix)) = stack.pop() {
            let mut entries = match tokio::fs::read_dir(&walk_dir).await {
                Ok(entries) => entries,
                Err(error) if is_not_found(&error) => continue,
                Err(error) => {
                    return Err(Error::storage(format!(
                        "Failed to list local storage under '{walk_prefix}': {error}"
                    )))
                }
            };
            loop {
                let entry = match entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(error) if is_not_found(&error) => break,
                    Err(error) => {
                        return Err(Error::storage(format!(
                            "Failed to list local storage under '{walk_prefix}': {error}"
                        )))
                    }
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
                if !is_dir && name.contains(TMP_MARKER) {
                    continue;
                }
                let child_key = if walk_prefix.is_empty() {
                    name.clone()
                } else {
                    format!("{walk_prefix}/{name}")
                };
                if is_dir {
                    stack.push((entry.path(), child_key));
                } else {
                    found.push(child_key);
                }
            }
        }
        Ok(found)
    }
}

impl Default for LocalFileStorage {
    fn default() -> Self {
        Self::new("./.strands/")
    }
}

fn normalize_base_dir(base_dir: &str) -> String {
    let mut out = String::with_capacity(base_dir.len());
    let mut prev_slash = false;
    for c in base_dir.chars() {
        if c == '/' && prev_slash {
            continue;
        }
        prev_slash = c == '/';
        out.push(c);
    }
    if out.len() > 1 && out.ends_with('/') {
        out.pop();
    }
    out
}

fn is_not_found(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound || error.raw_os_error() == Some(20)
}

#[async_trait]
impl Storage for LocalFileStorage {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        let normalized = normalize_key(key)?;
        let path = self.path_for(&normalized);
        let tmp_path = PathBuf::from(format!("{}{TMP_MARKER}_{}", path.display(), uuid::Uuid::new_v4()));
        match atomic_write(&path, &tmp_path, data).await {
            Ok(()) => {}
            Err(error) => {
                let _ = tokio::fs::remove_file(&tmp_path).await;
                return Err(Error::storage(format!(
                    "Failed to write '{normalized}' to local storage: {error}"
                )));
            }
        }
        if let Some(strategy) = &self.search_strategy {
            strategy.index(self, &normalized, data).await?;
        }
        Ok(())
    }

    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let normalized = normalize_key(key)?;
        match tokio::fs::read(self.path_for(&normalized)).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if is_not_found(&error) => Ok(None),
            Err(error) => Err(Error::storage(format!(
                "Failed to read '{normalized}' from local storage: {error}"
            ))),
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let normalized = normalize_key(key)?;
        match tokio::fs::remove_file(self.path_for(&normalized)).await {
            Ok(()) => Ok(()),
            Err(error) if is_not_found(&error) => Ok(()),
            Err(error) => Err(Error::storage(format!(
                "Failed to delete '{normalized}' from local storage: {error}"
            ))),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let normalized = normalize_prefix(prefix)?;
        let dir_portion = match normalized.rfind('/') {
            Some(i) => &normalized[..i],
            None => "",
        };
        let start_dir = if dir_portion.is_empty() {
            PathBuf::from(&self.base_dir)
        } else {
            PathBuf::from(format!("{}/{}", self.base_dir, dir_portion))
        };
        let mut keys = self
            .list_keys(start_dir, dir_portion.to_owned())
            .await?
            .into_iter()
            .filter(|key| key.starts_with(&normalized))
            .collect::<Vec<_>>();
        keys.sort();
        Ok(keys)
    }

    fn is_namespaced(&self) -> bool {
        self.namespaced
    }

    fn namespace(self: Arc<Self>, prefix: &str) -> Option<StorageRef> {
        Some(Arc::new(self.namespaced(prefix)) as StorageRef)
    }

    async fn search(&self, query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        Some(match &self.search_strategy {
            Some(strategy) => strategy.search(self, query).await,
            None => KeywordSearchStrategy.search(self, query).await,
        })
    }
}

async fn atomic_write(path: &Path, tmp_path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    tokio::fs::write(tmp_path, data).await?;
    tokio::fs::rename(tmp_path, path).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn storage() -> (TempDir, LocalFileStorage) {
        let dir = TempDir::new().unwrap();
        let storage = LocalFileStorage::new(dir.path().to_string_lossy().into_owned());
        (dir, storage)
    }

    #[tokio::test]
    async fn round_trips_bytes() {
        let (_dir, s) = storage();
        s.write("test/file.txt", b"hello world").await.unwrap();
        assert_eq!(
            s.read("test/file.txt").await.unwrap(),
            Some(b"hello world".to_vec())
        );
    }

    #[tokio::test]
    async fn creates_nested_directories() {
        let (dir, s) = storage();
        s.write("a/b/c/d.json", b"{}").await.unwrap();
        assert!(dir.path().join("a/b/c/d.json").exists());
        assert_eq!(s.read("a/b/c/d.json").await.unwrap(), Some(b"{}".to_vec()));
    }

    #[tokio::test]
    async fn overwrites_existing_values() {
        let (_dir, s) = storage();
        s.write("k", b"first").await.unwrap();
        s.write("k", b"second").await.unwrap();
        assert_eq!(s.read("k").await.unwrap(), Some(b"second".to_vec()));
    }

    #[tokio::test]
    async fn returns_null_for_missing_keys() {
        let (_dir, s) = storage();
        assert_eq!(s.read("nope").await.unwrap(), None);
    }

    #[tokio::test]
    async fn writes_atomically_via_tmp_file() {
        let (dir, s) = storage();
        s.write("atomic/test", &[1]).await.unwrap();
        assert_eq!(
            tokio::fs::read(dir.path().join("atomic/test")).await.unwrap(),
            vec![1]
        );
        let mut entries = tokio::fs::read_dir(dir.path().join("atomic")).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert!(!entry.file_name().to_string_lossy().contains(TMP_MARKER));
        }
    }

    #[tokio::test]
    async fn cleans_up_tmp_file_on_rename_failure() {
        let (dir, s) = storage();
        let file = dir.path().join("blocked");
        tokio::fs::write(&file, b"x").await.unwrap();
        let err = s.write("blocked/child", b"y").await.unwrap_err();
        assert!(err
            .to_string()
            .starts_with("Failed to write 'blocked/child' to local storage"));
        let mut entries = tokio::fs::read_dir(dir.path()).await.unwrap();
        while let Some(entry) = entries.next_entry().await.unwrap() {
            assert!(!entry.file_name().to_string_lossy().contains(TMP_MARKER));
        }
    }

    #[tokio::test]
    async fn rejects_keys_containing_backslashes() {
        let (_dir, s) = storage();
        assert!(s.write("bad\\key", b"x").await.is_err());
    }

    #[tokio::test]
    async fn rejects_backslash_path_traversal() {
        let (_dir, s) = storage();
        assert!(s.write("..\\..\\etc\\passwd", b"x").await.is_err());
    }

    #[tokio::test]
    async fn rejects_dot_dot_path_traversal() {
        let (_dir, s) = storage();
        let err = s.write("../escaped", b"x").await.unwrap_err();
        assert_eq!(
            err.to_string(),
            "Invalid storage key '../escaped': '..' path segments are not allowed"
        );
    }

    #[tokio::test]
    async fn removes_an_existing_key() {
        let (_dir, s) = storage();
        s.write("k", b"v").await.unwrap();
        s.delete("k").await.unwrap();
        assert_eq!(s.read("k").await.unwrap(), None);
    }

    #[tokio::test]
    async fn delete_is_a_no_op_for_missing_keys() {
        let (_dir, s) = storage();
        s.delete("missing").await.unwrap();
    }

    #[tokio::test]
    async fn lists_keys_under_a_prefix() {
        let (_dir, s) = storage();
        s.write("a/1", b"x").await.unwrap();
        s.write("a/2", b"y").await.unwrap();
        s.write("b/1", b"z").await.unwrap();
        assert_eq!(s.list("a/").await.unwrap(), vec!["a/1", "a/2"]);
    }

    #[tokio::test]
    async fn returns_all_keys_for_empty_prefix() {
        let (_dir, s) = storage();
        s.write("a/1", b"x").await.unwrap();
        s.write("b", b"y").await.unwrap();
        assert_eq!(s.list("").await.unwrap(), vec!["a/1", "b"]);
    }

    #[tokio::test]
    async fn returns_empty_array_when_base_directory_does_not_exist() {
        let dir = TempDir::new().unwrap();
        let s = LocalFileStorage::new(dir.path().join("nope").to_string_lossy().into_owned());
        assert!(s.list("").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn excludes_scratch_files() {
        let (dir, s) = storage();
        s.write("real", b"x").await.unwrap();
        tokio::fs::write(dir.path().join("real.__strands_tmp_abc"), b"junk")
            .await
            .unwrap();
        assert_eq!(s.list("").await.unwrap(), vec!["real"]);
    }

    #[tokio::test]
    async fn does_not_exclude_user_tmp_files() {
        let (_dir, s) = storage();
        s.write("notes.tmp", b"x").await.unwrap();
        assert_eq!(s.list("").await.unwrap(), vec!["notes.tmp"]);
    }

    #[tokio::test]
    async fn returns_keys_sorted_lexicographically() {
        let (_dir, s) = storage();
        for key in ["c", "a", "b/z", "b/a"] {
            s.write(key, b"x").await.unwrap();
        }
        assert_eq!(s.list("").await.unwrap(), vec!["a", "b/a", "b/z", "c"]);
    }

    #[tokio::test]
    async fn namespace_returns_a_local_file_storage_rooted_at_the_subdirectory() {
        let (dir, s) = storage();
        let scoped = s.namespaced("sub");
        scoped.write("k", b"v").await.unwrap();
        assert!(dir.path().join("sub/k").exists());
        assert!(scoped.is_namespaced());
    }

    #[tokio::test]
    async fn namespace_isolates_keys_from_the_parent_storage() {
        let (_dir, s) = storage();
        let scoped = s.namespaced("sub");
        scoped.write("k", b"v").await.unwrap();
        s.write("k", b"parent").await.unwrap();
        assert_eq!(scoped.list("").await.unwrap(), vec!["k"]);
        assert_eq!(s.list("").await.unwrap(), vec!["k", "sub/k"]);
        assert_eq!(scoped.read("k").await.unwrap(), Some(b"v".to_vec()));
    }

    #[tokio::test]
    async fn namespace_exposes_base_dir_pointing_to_the_subdirectory() {
        let (dir, s) = storage();
        let scoped = s.namespaced("sub/");
        assert_eq!(scoped.base_dir(), format!("{}/sub", dir.path().to_string_lossy()));
    }

    #[tokio::test]
    async fn uses_the_default_keyword_scan_without_a_strategy() {
        let (_dir, s) = storage();
        s.write("notes", b"dark mode toggle").await.unwrap();
        let hits = s.search("dark mode").await.unwrap().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, 2.0);
    }

    #[tokio::test]
    async fn indexes_writes_and_delegates_search_to_a_search_strategy() {
        let (_dir, s) = storage();
        let s = s.with_search_strategy(RecordingStrategy::default());
        s.write("k", b"v").await.unwrap();
        let hits = s.search("q").await.unwrap().unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "indexed:k");
    }

    #[tokio::test]
    async fn carries_a_search_strategy_through_namespace() {
        let (_dir, s) = storage();
        let s = s.with_search_strategy(RecordingStrategy::default());
        let scoped = s.namespaced("sub");
        scoped.write("k", b"v").await.unwrap();
        assert_eq!(scoped.search("q").await.unwrap().unwrap()[0].key, "indexed:k");
    }

    #[tokio::test]
    async fn base_dir_collapses_slash_runs_and_strips_trailing_slash() {
        assert_eq!(LocalFileStorage::new("/tmp//a/").base_dir(), "/tmp/a");
        assert_eq!(LocalFileStorage::new("/").base_dir(), "/");
        assert_eq!(LocalFileStorage::default().base_dir(), "./.strands");
    }

    #[derive(Default)]
    struct RecordingStrategy {
        indexed: parking_lot::Mutex<Vec<String>>,
    }

    #[async_trait]
    impl SearchStrategy for RecordingStrategy {
        async fn search(&self, _storage: &dyn Storage, _query: &str) -> Result<Vec<StorageSearchResult>> {
            Ok(self
                .indexed
                .lock()
                .iter()
                .map(|key| StorageSearchResult {
                    key: format!("indexed:{key}"),
                    score: 1.0,
                    data: None,
                })
                .collect())
        }

        async fn index(&self, _storage: &dyn Storage, key: &str, _data: &[u8]) -> Result<()> {
            self.indexed.lock().push(key.to_owned());
            Ok(())
        }
    }
}
