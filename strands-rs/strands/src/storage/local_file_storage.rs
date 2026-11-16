//! File-system storage (TS `storage/local-file-storage.ts`).

use std::fmt;
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;

use super::search::{KeywordSearchStrategy, SearchStrategy, StorageSearchResult};
use super::{Storage, normalize_key, normalize_prefix, sort_keys};
use crate::error::{Error, Result, StorageError};
use crate::sandbox::Sandbox;

const DEFAULT_BASE_DIR: &str = "./.strands/";
const TMP_MARKER: &str = ".__strands_tmp";

/// Configuration for [`LocalFileStorage`] (TS `LocalFileStorageConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
pub struct LocalFileStorageConfig {
    /// Routes every read, write, delete and listing through this sandbox instead of the host.
    pub sandbox: Option<Arc<dyn Sandbox>>,
    /// Indexes host writes and answers `search` instead of the keyword scan.
    pub search_strategy: Option<Arc<dyn SearchStrategy<LocalFileStorage>>>,
}

impl fmt::Debug for LocalFileStorageConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalFileStorageConfig")
            .field("sandbox", &self.sandbox.as_ref().map(|_| ".."))
            .field("search_strategy", &self.search_strategy.as_ref().map(|_| ".."))
            .finish()
    }
}

/// [`Storage`] as files under a base directory (TS `LocalFileStorage`): each key is a path
/// relative to it. Host writes are atomic (a temp file, then a rename).
///
/// ```no_run
/// use strands::storage::{LocalFileStorage, Storage};
///
/// # async fn run() -> strands::Result<()> {
/// let sessions = LocalFileStorage::new("./.agent").namespace("sessions")?;
/// sessions.write("notes.json", b"[]").await?;
/// # Ok(()) }
/// ```
#[derive(Clone)]
pub struct LocalFileStorage {
    base_dir: String,
    sandbox: Option<Arc<dyn Sandbox>>,
    search_strategy: Option<Arc<dyn SearchStrategy<LocalFileStorage>>>,
    namespaced: bool,
}

impl fmt::Debug for LocalFileStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LocalFileStorage")
            .field("base_dir", &self.base_dir)
            .field("sandboxed", &self.sandbox.is_some())
            .finish_non_exhaustive()
    }
}

impl Default for LocalFileStorage {
    /// Storage under `./.strands`.
    fn default() -> Self {
        Self::new(DEFAULT_BASE_DIR)
    }
}

impl LocalFileStorage {
    /// Storage under `base_dir` on the host, with the keyword search.
    pub fn new(base_dir: impl Into<String>) -> Self {
        Self::with_config(base_dir, LocalFileStorageConfig::default())
    }

    /// Storage under `base_dir` with `config`.
    pub fn with_config(base_dir: impl Into<String>, config: LocalFileStorageConfig) -> Self {
        Self {
            base_dir: normalize_base_dir(&base_dir.into()),
            sandbox: config.sandbox,
            search_strategy: config.search_strategy,
            namespaced: false,
        }
    }

    /// The normalized base directory.
    pub fn base_dir(&self) -> &str {
        &self.base_dir
    }

    /// The same storage routed through `sandbox` (TS `forSandbox`); `self` when already
    /// sandboxed.
    #[must_use]
    pub fn for_sandbox(&self, sandbox: Arc<dyn Sandbox>) -> Self {
        if self.sandbox.is_some() {
            return self.clone();
        }
        Self {
            sandbox: Some(sandbox),
            ..self.clone()
        }
    }

    /// Storage under `<base_dir>/<prefix>` with the same sandbox and search strategy.
    ///
    /// # Errors
    ///
    /// The prefix has a backslash or a `..` segment.
    pub fn namespace(&self, prefix: &str) -> Result<Self> {
        let normalized = normalize_prefix(prefix)?;
        let base_dir = if normalized.is_empty() {
            self.base_dir.clone()
        } else {
            format!("{}/{normalized}", self.base())
        };
        Ok(Self {
            base_dir: normalize_base_dir(&base_dir),
            sandbox: self.sandbox.clone(),
            search_strategy: self.search_strategy.clone(),
            namespaced: true,
        })
    }

    /// The matches for `query` from the configured strategy, else the keyword scan.
    ///
    /// # Errors
    ///
    /// The strategy or a listing failed.
    pub async fn search(&self, query: &str) -> Result<Vec<StorageSearchResult>> {
        match &self.search_strategy {
            Some(strategy) => strategy.search(self, query).await,
            None => KeywordSearchStrategy.search(self, query).await,
        }
    }

    fn base(&self) -> &str {
        self.base_dir.strip_suffix('/').unwrap_or(&self.base_dir)
    }

    fn path_for(&self, key: &str) -> String {
        format!("{}/{key}", self.base())
    }

    async fn write_host(path: &str, data: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = Path::new(path).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let tmp_path = format!("{path}{TMP_MARKER}_{}", uuid::Uuid::now_v7());
        let result = async {
            tokio::fs::write(&tmp_path, data).await?;
            tokio::fs::rename(&tmp_path, path).await
        }
        .await;
        if result.is_err() {
            let _ = tokio::fs::remove_file(&tmp_path).await;
        }
        result
    }

    async fn list_host(dir: String, key_prefix: String) -> Result<Vec<String>> {
        let mut found = Vec::new();
        let mut pending = vec![(dir, key_prefix)];
        while let Some((dir, prefix)) = pending.pop() {
            let mut entries = match tokio::fs::read_dir(&dir).await {
                Ok(entries) => entries,
                Err(error) if is_not_found(&error) => continue,
                Err(error) => return Err(list_error("local", &prefix, error)),
            };
            loop {
                let entry = match entries.next_entry().await {
                    Ok(Some(entry)) => entry,
                    Ok(None) => break,
                    Err(error) => return Err(list_error("local", &prefix, error)),
                };
                let name = entry.file_name().to_string_lossy().into_owned();
                let is_dir = entry
                    .file_type()
                    .await
                    .map_err(|error| list_error("local", &prefix, error))?
                    .is_dir();
                visit(&mut found, &mut pending, &dir, &prefix, &name, is_dir);
            }
        }
        Ok(found)
    }

    async fn list_sandbox(sandbox: &dyn Sandbox, dir: String, key_prefix: String) -> Result<Vec<String>> {
        let mut found = Vec::new();
        let mut pending = vec![(dir, key_prefix)];
        while let Some((dir, prefix)) = pending.pop() {
            let entries = match sandbox.list_files(&dir).await {
                Ok(entries) => entries,
                Err(error) if is_sandbox_not_found(&error) => continue,
                Err(error) => return Err(list_error("sandbox", &prefix, error)),
            };
            for entry in entries {
                let is_dir = entry.is_dir.unwrap_or(false);
                visit(&mut found, &mut pending, &dir, &prefix, &entry.name, is_dir);
            }
        }
        Ok(found)
    }
}

/// Collapses `/` runs and drops one trailing `/` (TS constructor).
fn normalize_base_dir(base_dir: &str) -> String {
    let mut collapsed = String::with_capacity(base_dir.len());
    for c in base_dir.chars() {
        if c == '/' && collapsed.ends_with('/') {
            continue;
        }
        collapsed.push(c);
    }
    if collapsed.len() > 1 && collapsed.ends_with('/') {
        collapsed.pop();
    }
    collapsed
}

fn visit(
    found: &mut Vec<String>,
    pending: &mut Vec<(String, String)>,
    dir: &str,
    prefix: &str,
    name: &str,
    is_dir: bool,
) {
    if !is_dir && name.contains(TMP_MARKER) {
        return;
    }
    let child_key = if prefix.is_empty() {
        name.to_owned()
    } else {
        format!("{prefix}/{name}")
    };
    if is_dir {
        pending.push((format!("{dir}/{name}"), child_key));
    } else {
        found.push(child_key);
    }
}

fn is_not_found(error: &std::io::Error) -> bool {
    matches!(error.kind(), ErrorKind::NotFound | ErrorKind::NotADirectory)
}

fn is_sandbox_not_found(error: &Error) -> bool {
    match error {
        Error::SandboxPathNotFound(_) => true,
        Error::Other(source) => source.downcast_ref::<std::io::Error>().is_some_and(is_not_found),
        _ => false,
    }
}

fn storage_error(message: String, source: impl Into<crate::error::BoxError>) -> Error {
    StorageError::new(message).with_source(source).into()
}

fn list_error(kind: &str, prefix: &str, source: impl Into<crate::error::BoxError>) -> Error {
    storage_error(format!("Failed to list {kind} storage under '{prefix}'"), source)
}

#[async_trait]
impl Storage for LocalFileStorage {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        let normalized = normalize_key(key)?;
        let path = self.path_for(&normalized);
        if let Some(sandbox) = &self.sandbox {
            return sandbox.write_file(&path, data).await.map_err(|error| {
                storage_error(
                    format!("Failed to write '{normalized}' to sandbox storage"),
                    error,
                )
            });
        }
        Self::write_host(&path, data).await.map_err(|error| {
            storage_error(format!("Failed to write '{normalized}' to local storage"), error)
        })?;
        if let Some(strategy) = &self.search_strategy {
            strategy.index(self, &normalized, data).await?;
        }
        Ok(())
    }

    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let normalized = normalize_key(key)?;
        let path = self.path_for(&normalized);
        if let Some(sandbox) = &self.sandbox {
            return match sandbox.read_file(&path).await {
                Ok(bytes) => Ok(Some(bytes)),
                Err(error) if is_sandbox_not_found(&error) => Ok(None),
                Err(error) => Err(storage_error(
                    format!("Failed to read '{normalized}' from sandbox storage"),
                    error,
                )),
            };
        }
        match tokio::fs::read(&path).await {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if is_not_found(&error) => Ok(None),
            Err(error) => Err(storage_error(
                format!("Failed to read '{normalized}' from local storage"),
                error,
            )),
        }
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let normalized = normalize_key(key)?;
        let path = self.path_for(&normalized);
        if let Some(sandbox) = &self.sandbox {
            return match sandbox.remove_file(&path).await {
                Err(error) if !is_sandbox_not_found(&error) => Err(storage_error(
                    format!("Failed to delete '{normalized}' from sandbox storage"),
                    error,
                )),
                _ => Ok(()),
            };
        }
        match tokio::fs::remove_file(&path).await {
            Err(error) if !is_not_found(&error) => Err(storage_error(
                format!("Failed to delete '{normalized}' from local storage"),
                error,
            )),
            _ => Ok(()),
        }
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let normalized = normalize_prefix(prefix)?;
        let dir_portion = normalized.rsplit_once('/').map_or("", |(dir, _)| dir);
        let start_dir = if dir_portion.is_empty() {
            self.base().to_owned()
        } else {
            format!("{}/{dir_portion}", self.base())
        };
        let keys = match &self.sandbox {
            Some(sandbox) => Self::list_sandbox(&**sandbox, start_dir, dir_portion.to_owned()).await?,
            None => Self::list_host(start_dir, dir_portion.to_owned()).await?,
        };
        let mut keys: Vec<String> = keys
            .into_iter()
            .filter(|key| key.starts_with(&normalized))
            .collect();
        sort_keys(&mut keys);
        Ok(keys)
    }

    fn namespace(&self, prefix: &str) -> Option<Result<Arc<dyn Storage>>> {
        Some(LocalFileStorage::namespace(self, prefix).map(|scoped| Arc::new(scoped) as Arc<dyn Storage>))
    }

    async fn search(&self, query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        Some(LocalFileStorage::search(self, query).await)
    }

    fn storage_type_name(&self) -> &'static str {
        "LocalFileStorage"
    }

    fn is_namespaced(&self) -> bool {
        self.namespaced
    }
}

#[cfg(test)]
mod tests;
