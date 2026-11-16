//! Search over stored content (TS `storage/search/`).

mod keyword;

use async_trait::async_trait;

pub use keyword::KeywordSearchStrategy;

use super::Storage;
use crate::error::Result;

/// One match from a search (TS `StorageSearchResult`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct StorageSearchResult {
    /// The storage key of the match.
    pub key: String,
    /// Relevance; higher is more relevant.
    pub score: f64,
    /// The stored bytes, when the backend includes them.
    pub data: Option<Vec<u8>>,
}

impl StorageSearchResult {
    /// A result without data.
    pub fn new(key: impl Into<String>, score: f64) -> Self {
        Self {
            key: key.into(),
            score,
            data: None,
        }
    }

    /// Attaches the stored bytes.
    #[must_use]
    pub fn with_data(mut self, data: Vec<u8>) -> Self {
        self.data = Some(data);
        self
    }
}

/// A pluggable search over a storage backend (TS `SearchStrategy`).
///
/// A strategy shared by namespaced views receives each view with keys relative to its prefix,
/// so an index it keeps must not be keyed by those relative keys alone.
#[async_trait]
pub trait SearchStrategy<S: ?Sized + Sync = dyn Storage>: Send + Sync + 'static {
    /// Indexes an entry after the backend wrote it; the default does nothing.
    async fn index(&self, storage: &S, key: &str, data: &[u8]) -> Result<()> {
        let _ = (storage, key, data);
        Ok(())
    }

    /// The matches for `query`, ranked best-first.
    async fn search(&self, storage: &S, query: &str) -> Result<Vec<StorageSearchResult>>;
}
