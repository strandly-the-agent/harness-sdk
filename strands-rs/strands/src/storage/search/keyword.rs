//! Keyword search by token overlap (TS `storage/search/keyword.ts`).

use std::collections::HashSet;
use std::sync::LazyLock;

use async_trait::async_trait;
use regex::Regex;

use super::{SearchStrategy, StorageSearchResult};
use crate::error::Result;
use crate::storage::Storage;

/// Entries read concurrently per batch, to avoid throttling remote backends.
const READ_BATCH_SIZE: usize = 8;

static TOKEN_SEPARATOR: LazyLock<Option<Regex>> = LazyLock::new(|| Regex::new(r"[^\p{L}\p{N}_]+").ok());

const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had", "do", "does",
    "did", "will", "would", "could", "should", "may", "might", "shall", "can", "need", "dare", "ought",
    "used", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as", "into", "through", "during",
    "before", "after", "above", "below", "between", "out", "off", "over", "under", "again", "further",
    "then", "once", "and", "but", "or", "nor", "not", "so", "yet", "both", "either", "neither", "each",
    "every", "all", "any", "few", "more", "most", "other", "some", "such", "no", "only", "own", "same",
    "than", "too", "very", "just", "because", "about", "up", "its", "it", "he", "she", "they", "we", "you",
    "i", "me", "him", "her", "us", "them", "my", "your", "his", "our", "their", "this", "that", "these",
    "those", "what", "which", "who", "whom", "when", "where", "why", "how", "if", "while", "although",
    "though", "unless", "until", "since",
];

/// Keyword search using token-overlap scoring (TS `KeywordSearchStrategy`), the default for the
/// shipped backends.
///
/// Scores each entry (its key without the file extension, plus its content) by the number of
/// distinct query tokens it contains, ignoring stop words in the query. Entries are read in
/// batches of 8; each result carries the bytes read, and entries whose read fails are skipped.
///
/// ```
/// use strands::storage::search::{KeywordSearchStrategy, SearchStrategy};
/// use strands::storage::{InMemoryStorage, Storage};
///
/// # tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(async {
/// let storage = InMemoryStorage::new();
/// storage.write("notes/theme.md", b"dark mode toggle").await?;
/// let results = KeywordSearchStrategy.search(&storage, "dark mode").await?;
/// assert_eq!(results[0].key, "notes/theme.md");
/// # Ok::<(), strands::Error>(()) }).unwrap();
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "a unit value used as `KeywordSearchStrategy`, like the TS object"
)]
#[derive(Debug, Clone, Copy, Default)]
pub struct KeywordSearchStrategy;

#[async_trait]
impl<S: Storage + ?Sized> SearchStrategy<S> for KeywordSearchStrategy {
    async fn search(&self, storage: &S, query: &str) -> Result<Vec<StorageSearchResult>> {
        let query_tokens: HashSet<String> = tokenize(query)
            .into_iter()
            .filter(|token| !STOP_WORDS.contains(&token.as_str()))
            .collect();
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }
        let keys = storage.list("").await?;
        let mut scored = Vec::new();
        for batch in keys.chunks(READ_BATCH_SIZE) {
            let reads = futures::future::join_all(batch.iter().map(|key| read_or_skip(storage, key))).await;
            for (key, bytes) in batch.iter().zip(reads) {
                let Some(bytes) = bytes else { continue };
                let text = format!("{} {}", strip_extension(key), String::from_utf8_lossy(&bytes));
                let score = token_overlap_score(&query_tokens, &text);
                if score > 0 {
                    scored.push(StorageSearchResult::new(key.clone(), f64::from(score)).with_data(bytes));
                }
            }
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(scored)
    }
}

/// Lowercased word tokens of `text` (TS `tokenize`).
fn tokenize(text: &str) -> HashSet<String> {
    let lower = text.to_lowercase();
    let Some(separator) = TOKEN_SEPARATOR.as_ref() else {
        return HashSet::new();
    };
    separator
        .split(&lower)
        .filter(|token| !token.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Distinct query tokens that appear in `content` (TS `tokenOverlapScore`).
fn token_overlap_score(query_tokens: &HashSet<String>, content: &str) -> u32 {
    let matches = tokenize(content)
        .iter()
        .filter(|token| query_tokens.contains(*token))
        .count();
    u32::try_from(matches).unwrap_or(u32::MAX)
}

/// `key` without its file extension, so `md` isn't scored on every `.md` entry (TS
/// `stripExtension`, `/(?<=[^/])\.[^./]*$/`).
fn strip_extension(key: &str) -> &str {
    let Some((stem, extension)) = key.rsplit_once('.') else {
        return key;
    };
    if extension.contains('/') || stem.is_empty() || stem.ends_with('/') {
        return key;
    }
    stem
}

async fn read_or_skip<S: Storage + ?Sized>(storage: &S, key: &str) -> Option<Vec<u8>> {
    match storage.read(key).await {
        Ok(bytes) => bytes,
        Err(error) => {
            tracing::debug!("key=<{key}>, error=<{error}> | skipped unreadable entry");
            None
        }
    }
}

#[cfg(test)]
mod tests;
