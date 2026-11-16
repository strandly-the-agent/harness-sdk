//! In-memory storage (TS `storage/in-memory-storage.ts`).

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;

use super::search::{KeywordSearchStrategy, SearchStrategy, StorageSearchResult};
use super::{Storage, namespace_prefix, normalize_key, normalize_prefix, sort_keys};
use crate::error::Result;

/// Configuration for [`InMemoryStorage`] (TS `InMemoryStorageConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, Default, bon::Builder)]
pub struct InMemoryStorageConfig {
    /// Indexes every write and answers `search` instead of the keyword scan.
    pub search_strategy: Option<Arc<dyn SearchStrategy<InMemoryStorage>>>,
}

impl fmt::Debug for InMemoryStorageConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryStorageConfig")
            .field("search_strategy", &self.search_strategy.as_ref().map(|_| ".."))
            .finish()
    }
}

/// Unbounded in-memory [`Storage`] (TS `InMemoryStorage`), for tests and environments without a
/// disk. Content doesn't survive the process. Clones and [`namespace`](Self::namespace) views
/// share the entries.
///
/// ```
/// use strands::storage::{InMemoryStorage, Storage};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let storage = InMemoryStorage::new();
/// storage.write("memory/notes.json", b"[]").await?;
/// assert_eq!(storage.read("memory/notes.json").await?, Some(b"[]".to_vec()));
/// # Ok::<(), strands::Error>(()) }).unwrap();
/// ```
#[derive(Clone, Default)]
pub struct InMemoryStorage {
    store: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    prefix: String,
    search_strategy: Option<Arc<dyn SearchStrategy<InMemoryStorage>>>,
    namespaced: bool,
}

impl fmt::Debug for InMemoryStorage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InMemoryStorage")
            .field("prefix", &self.prefix)
            .finish_non_exhaustive()
    }
}

impl InMemoryStorage {
    /// Empty storage with the keyword search.
    pub fn new() -> Self {
        Self::default()
    }

    /// Empty storage with `config`.
    pub fn with_config(config: InMemoryStorageConfig) -> Self {
        Self {
            search_strategy: config.search_strategy,
            ..Self::default()
        }
    }

    /// A view under `prefix` that shares this storage's entries and search strategy. Its search
    /// sees only entries under the prefix and scores keys without it.
    ///
    /// # Errors
    ///
    /// The prefix has a backslash or a `..` segment.
    pub fn namespace(&self, prefix: &str) -> Result<Self> {
        Ok(Self {
            store: Arc::clone(&self.store),
            prefix: format!("{}{}", self.prefix, namespace_prefix(prefix)?),
            search_strategy: self.search_strategy.clone(),
            namespaced: true,
        })
    }

    /// The matches for `query` from the configured strategy, else the keyword scan.
    ///
    /// # Errors
    ///
    /// The strategy failed.
    pub async fn search(&self, query: &str) -> Result<Vec<StorageSearchResult>> {
        match &self.search_strategy {
            Some(strategy) => strategy.search(self, query).await,
            None => KeywordSearchStrategy.search(self, query).await,
        }
    }

    /// Removes every entry this instance sees (those under its namespace, or all).
    pub fn clear(&self) {
        let mut store = self.store.lock().unwrap_or_else(PoisonError::into_inner);
        if self.prefix.is_empty() {
            store.clear();
        } else {
            store.retain(|key, _| !key.starts_with(&self.prefix));
        }
    }

    fn full_key(&self, key: &str) -> Result<String> {
        Ok(format!("{}{}", self.prefix, normalize_key(key)?))
    }
}

#[async_trait]
impl Storage for InMemoryStorage {
    async fn write(&self, key: &str, data: &[u8]) -> Result<()> {
        let normalized = normalize_key(key)?;
        self.store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(format!("{}{normalized}", self.prefix), data.to_vec());
        if let Some(strategy) = &self.search_strategy {
            strategy.index(self, &normalized, data).await?;
        }
        Ok(())
    }

    async fn read(&self, key: &str) -> Result<Option<Vec<u8>>> {
        let key = self.full_key(key)?;
        Ok(self
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&key)
            .cloned())
    }

    async fn delete(&self, key: &str) -> Result<()> {
        let key = self.full_key(key)?;
        self.store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(&key);
        Ok(())
    }

    async fn list(&self, prefix: &str) -> Result<Vec<String>> {
        let full_prefix = format!("{}{}", self.prefix, normalize_prefix(prefix)?);
        let mut keys: Vec<String> = self
            .store
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .keys()
            .filter(|key| key.starts_with(&full_prefix))
            .filter_map(|key| key.get(self.prefix.len()..).map(str::to_owned))
            .collect();
        sort_keys(&mut keys);
        Ok(keys)
    }

    fn namespace(&self, prefix: &str) -> Option<Result<Arc<dyn Storage>>> {
        Some(InMemoryStorage::namespace(self, prefix).map(|scoped| Arc::new(scoped) as Arc<dyn Storage>))
    }

    async fn search(&self, query: &str) -> Option<Result<Vec<StorageSearchResult>>> {
        Some(InMemoryStorage::search(self, query).await)
    }

    fn storage_type_name(&self) -> &'static str {
        "InMemoryStorage"
    }

    fn is_namespaced(&self) -> bool {
        self.namespaced
    }

    fn is_ephemeral(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests;
