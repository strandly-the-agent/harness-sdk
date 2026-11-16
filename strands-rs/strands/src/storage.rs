//! Unified storage (TS `storage/storage.ts`): raw bytes under `/`-separated string keys, shared by
//! sessions, memory and context offloading.

mod in_memory_storage;
mod local_file_storage;
pub mod search;

use std::cmp::Ordering;
use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

pub use in_memory_storage::{InMemoryStorage, InMemoryStorageConfig};
pub use local_file_storage::{LocalFileStorage, LocalFileStorageConfig};
pub use search::StorageSearchResult;

use crate::error::{Result, StorageError};

/// A backend for storing and retrieving raw bytes under string keys (TS `Storage`).
///
/// Implementations round-trip bytes unchanged. The shipped backends treat `/` as a separator
/// (collapsing runs, rejecting `..`); custom backends may use their own key scheme.
#[async_trait]
pub trait Storage: Send + Sync + 'static {
    /// Stores `data` under `key`, overwriting any existing value.
    async fn write(&self, key: &str, data: &[u8]) -> Result<()>;

    /// The bytes stored under `key`, or `None` when no value exists.
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>>;

    /// Deletes the value under `key`; a no-op when it doesn't exist.
    async fn delete(&self, key: &str) -> Result<()>;

    /// The full keys starting with `prefix`, sorted ascending; `""` lists every key.
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;

    /// A view with every key under `prefix` (TS optional `namespace`); `None` when the backend
    /// has no native namespacing, so callers fall back to a generic prefixed view.
    fn namespace(&self, prefix: &str) -> Option<Result<Arc<dyn Storage>>> {
        let _ = prefix;
        None
    }

    /// Matches for `query`, ranked best-first (TS optional `search`); `None` when unsupported.
    async fn search(&self, query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        let _ = query;
        None
    }

    /// Whether this is already a namespaced view (TS `NAMESPACED` marker).
    #[doc(hidden)]
    fn is_namespaced(&self) -> bool {
        false
    }

    /// Whether the data is lost when the process exits (TS `EPHEMERAL` marker).
    #[doc(hidden)]
    fn is_ephemeral(&self) -> bool {
        false
    }
}

impl fmt::Debug for dyn Storage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Storage(..)")
    }
}

/// Normalizes a key for path-based backends (TS `normalizeKey`): collapses `/` runs, strips
/// leading and trailing `/`, and rejects backslashes, empty keys and `..` segments.
pub(crate) fn normalize_key(key: &str) -> Result<String> {
    if key.contains('\\') {
        return Err(StorageError::new(format!(
            "Invalid storage key '{key}': backslashes are not allowed"
        ))
        .into());
    }
    let segments: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(StorageError::new("Storage key must not be empty").into());
    }
    if segments.contains(&"..") {
        return Err(StorageError::new(format!(
            "Invalid storage key '{key}': '..' path segments are not allowed"
        ))
        .into());
    }
    Ok(segments.join("/"))
}

/// Normalizes a list prefix (TS `normalizePrefix`): collapses `/` runs and strips leading `/`,
/// keeping one trailing `/`. An empty prefix is valid and matches everything.
pub(crate) fn normalize_prefix(prefix: &str) -> Result<String> {
    if prefix.contains('\\') {
        return Err(StorageError::new(format!(
            "Invalid storage prefix '{prefix}': backslashes are not allowed"
        ))
        .into());
    }
    let segments: Vec<&str> = prefix.split('/').filter(|s| !s.is_empty()).collect();
    if segments.contains(&"..") {
        return Err(StorageError::new(format!(
            "Invalid storage prefix '{prefix}': '..' path segments are not allowed"
        ))
        .into());
    }
    let joined = segments.join("/");
    if prefix.ends_with('/') && !joined.is_empty() {
        return Ok(joined + "/");
    }
    Ok(joined)
}

/// The prefix a namespace view adds: the normalized prefix plus `/`, or `""`.
fn namespace_prefix(prefix: &str) -> Result<String> {
    let normalized = normalize_prefix(prefix)?;
    let normalized = normalized.trim_end_matches('/');
    Ok(if normalized.is_empty() {
        String::new()
    } else {
        format!("{normalized}/")
    })
}

/// Sorts keys the way JS `Array.prototype.sort` does (UTF-16 code units).
pub(crate) fn sort_keys(keys: &mut [String]) {
    keys.sort_by(|a, b| js_cmp(a, b));
}

fn js_cmp(a: &str, b: &str) -> Ordering {
    a.encode_utf16().cmp(b.encode_utf16())
}

/// A generic prefixed view over `storage` (TS `namespace`); composes by nesting prefixes.
pub(crate) fn namespace(storage: Arc<dyn Storage>, prefix: &str) -> Result<Arc<dyn Storage>> {
    let prefix = namespace_prefix(prefix)?;
    Ok(Arc::new(NamespacedStorage {
        inner: storage,
        prefix,
    }))
}
struct NamespacedStorage {
    inner: Arc<dyn Storage>,
    prefix: String,
}

impl NamespacedStorage {
    fn strip<'a>(&self, key: &'a str) -> &'a str {
        key.strip_prefix(self.prefix.as_str()).unwrap_or(key)
    }
}

#[async_trait]
impl Storage for NamespacedStorage {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        self.inner.write(&format!("{}{key}", self.prefix), data).await
    }

    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.read(&format!("{}{key}", self.prefix)).await
    }

    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(&format!("{}{key}", self.prefix)).await
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let keys = self.inner.list(&format!("{}{prefix}", self.prefix)).await?;
        Ok(keys.iter().map(|key| self.strip(key).to_owned()).collect())
    }

    fn namespace(&self, prefix: &str) -> Option<Result<Arc<dyn Storage>>> {
        Some(namespace(
            Arc::clone(&self.inner),
            &format!("{}{prefix}", self.prefix),
        ))
    }

    async fn search(&self, query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        let results = self.inner.search(query).await?;
        Some(results.map(|results| {
            results
                .into_iter()
                .filter(|result| result.key.starts_with(self.prefix.as_str()))
                .map(|mut result| {
                    result.key = self.strip(&result.key).to_owned();
                    result
                })
                .collect()
        }))
    }

    fn is_namespaced(&self) -> bool {
        true
    }

    fn is_ephemeral(&self) -> bool {
        self.inner.is_ephemeral()
    }
}

#[cfg(test)]
mod tests;
