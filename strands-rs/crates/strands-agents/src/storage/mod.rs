//! Byte-oriented key/value storage (`strands-ts/src/storage/storage.ts`). Everything is bytes;
//! `namespace`/`search` are optional capabilities callers feature-detect.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::error::{Error, Result};

/// A hit from [`Storage::search`].
#[derive(Clone, Debug, PartialEq)]
pub struct StorageSearchResult {
    /// Key of the matching object.
    pub key: String,
    /// Relevance score (higher is better).
    pub score: f64,
    /// Object bytes, when the backend returns them.
    pub data: Option<Vec<u8>>,
}

/// Shared storage handle.
pub type StorageRef = Arc<dyn Storage>;

/// Byte storage keyed by `/`-separated paths.
#[async_trait]
pub trait Storage: Send + Sync {
    /// Write `data` under `key` (created or overwritten).
    async fn write(&self, key: &str, data: &[u8]) -> Result<()>;
    /// Read `key`; `None` when absent.
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>>;
    /// Delete `key`; no error when absent.
    async fn delete(&self, key: &str) -> Result<()>;
    /// Keys starting with `prefix` (see [`normalize_prefix`] for the trailing-slash rule).
    async fn list(&self, prefix: &str) -> Result<Vec<String>>;
    /// `true` when contents do not survive the process (`EPHEMERAL` marker in TS).
    fn is_ephemeral(&self) -> bool {
        false
    }
    /// `true` when this handle is already a namespaced view (`NAMESPACED` marker in TS).
    fn is_namespaced(&self) -> bool {
        false
    }
    /// Optional backend-native namespacing; `None` means "use the generic wrapper".
    fn namespace(self: Arc<Self>, _prefix: &str) -> Option<StorageRef> {
        None
    }
    /// Optional search; `None` means unsupported.
    async fn search(&self, _query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        None
    }
}

/// Validate and canonicalize a key: collapses `//`, strips leading/trailing `/`, rejects `\`
/// and `..`, rejects empty.
pub fn normalize_key(key: &str) -> Result<String> {
    if key.contains('\\') {
        return Err(Error::storage(format!(
            "Invalid storage key '{key}': backslashes are not allowed"
        )));
    }
    let segments: Vec<&str> = key.split('/').filter(|s| !s.is_empty()).collect();
    if segments.is_empty() {
        return Err(Error::storage("Storage key must not be empty"));
    }
    if segments.contains(&"..") {
        return Err(Error::storage(format!(
            "Invalid storage key '{key}': '..' path segments are not allowed"
        )));
    }
    Ok(segments.join("/"))
}

/// Validate and canonicalize a prefix: like [`normalize_key`] but empty is legal and a trailing
/// `/` is preserved when the body is non-empty (`"a/"` = inside `a/`, `"a"` = literal prefix).
pub fn normalize_prefix(prefix: &str) -> Result<String> {
    if prefix.contains('\\') {
        return Err(Error::storage(format!(
            "Invalid storage prefix '{prefix}': backslashes are not allowed"
        )));
    }
    let segments: Vec<&str> = prefix.split('/').filter(|s| !s.is_empty()).collect();
    if segments.contains(&"..") {
        return Err(Error::storage(format!(
            "Invalid storage prefix '{prefix}': '..' path segments are not allowed"
        )));
    }
    let mut out = segments.join("/");
    if !out.is_empty() && prefix.ends_with('/') {
        out.push('/');
    }
    Ok(out)
}

/// A prefix view over another storage.
struct Namespaced {
    inner: StorageRef,
    prefix: String,
}

#[async_trait]
impl Storage for Namespaced {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        self.inner.write(&format!("{}{}", self.prefix, key), data).await
    }
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        self.inner.read(&format!("{}{}", self.prefix, key)).await
    }
    async fn delete(&self, key: &str) -> Result<()> {
        self.inner.delete(&format!("{}{}", self.prefix, key)).await
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let keys = self.inner.list(&format!("{}{}", self.prefix, prefix)).await?;
        Ok(keys
            .into_iter()
            .map(|k| k[self.prefix.len()..].to_owned())
            .collect())
    }
    fn is_ephemeral(&self) -> bool {
        self.inner.is_ephemeral()
    }
    fn is_namespaced(&self) -> bool {
        true
    }
    fn namespace(self: Arc<Self>, prefix: &str) -> Option<StorageRef> {
        Some(namespace(
            Arc::clone(&self.inner),
            &format!("{}{}", self.prefix, prefix),
        ))
    }
    async fn search(&self, query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        let results = self.inner.search(query).await?;
        Some(results.map(|hits| {
            hits.into_iter()
                .filter(|h| h.key.starts_with(&self.prefix))
                .map(|mut h| {
                    h.key = h.key[self.prefix.len()..].to_owned();
                    h
                })
                .collect()
        }))
    }
}

/// Generic prefix view (`namespace(storage, prefix)` in TS). Composes.
pub fn namespace(storage: StorageRef, prefix: &str) -> StorageRef {
    let prefix = normalize_prefix(prefix).unwrap_or_default();
    let prefix = prefix.trim_end_matches('/').to_owned();
    let prefix = if prefix.is_empty() {
        String::new()
    } else {
        format!("{prefix}/")
    };
    Arc::new(Namespaced {
        inner: storage,
        prefix,
    })
}

/// Scope `storage` under `prefix` unless it is already a namespaced view; prefers the backend's
/// own `namespace` when it has one (`resolveNamespace` in TS).
pub fn resolve_namespace(storage: StorageRef, prefix: &str) -> StorageRef {
    if storage.is_namespaced() {
        return storage;
    }
    match Arc::clone(&storage).namespace(prefix) {
        Some(scoped) => scoped,
        None => namespace(storage, prefix),
    }
}

/// Process-local storage; contents are lost on exit (`EPHEMERAL`).
#[derive(Default)]
pub struct InMemoryStorage {
    map: RwLock<BTreeMap<String, Vec<u8>>>,
}

impl InMemoryStorage {
    /// Empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Remove everything.
    pub fn clear(&self) {
        self.map.write().clear();
    }
}

#[async_trait]
impl Storage for InMemoryStorage {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        self.map.write().insert(normalize_key(key)?, data.to_vec());
        Ok(())
    }
    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.map.read().get(&normalize_key(key)?).cloned())
    }
    async fn delete(&self, key: &str) -> Result<()> {
        self.map.write().remove(&normalize_key(key)?);
        Ok(())
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let prefix = normalize_prefix(prefix)?;
        Ok(self
            .map
            .read()
            .keys()
            .filter(|k| k.starts_with(&prefix))
            .cloned()
            .collect())
    }
    fn is_ephemeral(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_rules() {
        assert_eq!(normalize_key("/a//b/").unwrap(), "a/b");
        assert_eq!(
            normalize_key("").unwrap_err().to_string(),
            "Storage key must not be empty"
        );
        assert_eq!(
            normalize_key("a/../b").unwrap_err().to_string(),
            "Invalid storage key 'a/../b': '..' path segments are not allowed"
        );
        assert_eq!(
            normalize_key("a\\b").unwrap_err().to_string(),
            "Invalid storage key 'a\\b': backslashes are not allowed"
        );
        assert_eq!(normalize_key("./a").unwrap(), "./a");
    }

    #[test]
    fn prefix_rules() {
        assert_eq!(normalize_prefix("").unwrap(), "");
        assert_eq!(normalize_prefix("a/").unwrap(), "a/");
        assert_eq!(normalize_prefix("/a//b").unwrap(), "a/b");
        assert_eq!(normalize_prefix("/").unwrap(), "");
        assert!(normalize_prefix("a/..").is_err());
    }

    #[tokio::test]
    async fn in_memory_round_trip_and_list() {
        let s = InMemoryStorage::new();
        s.write("a/1", b"x").await.unwrap();
        s.write("a/2", b"y").await.unwrap();
        s.write("ab", b"z").await.unwrap();
        assert_eq!(s.read("a/1").await.unwrap(), Some(b"x".to_vec()));
        assert_eq!(s.read("missing").await.unwrap(), None);
        assert_eq!(s.list("a").await.unwrap(), vec!["a/1", "a/2", "ab"]);
        assert_eq!(s.list("a/").await.unwrap(), vec!["a/1", "a/2"]);
        s.delete("a/1").await.unwrap();
        s.delete("a/1").await.unwrap();
        assert_eq!(s.list("").await.unwrap(), vec!["a/2", "ab"]);
        assert!(s.is_ephemeral());
    }

    #[tokio::test]
    async fn namespace_prefixes_and_strips_keys_and_composes() {
        let root: StorageRef = Arc::new(InMemoryStorage::new());
        let ns = resolve_namespace(Arc::clone(&root), "agent/");
        ns.write("k", b"v").await.unwrap();
        assert_eq!(root.read("agent/k").await.unwrap(), Some(b"v".to_vec()));
        assert_eq!(ns.list("").await.unwrap(), vec!["k"]);
        assert!(ns.is_namespaced() && ns.is_ephemeral());
        let sub = Arc::clone(&ns).namespace("sub").unwrap();
        sub.write("x", b"1").await.unwrap();
        assert_eq!(root.list("agent/sub/").await.unwrap(), vec!["agent/sub/x"]);
        assert!(Arc::ptr_eq(&resolve_namespace(Arc::clone(&ns), "other"), &ns));
    }
}
