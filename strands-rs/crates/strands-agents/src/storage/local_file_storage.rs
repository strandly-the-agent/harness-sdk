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
        let tmp_path = PathBuf::from(format!(
            "{}{TMP_MARKER}_{}",
            path.display(),
            uuid::Uuid::new_v4()
        ));
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
