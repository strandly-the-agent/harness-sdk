//! Pluggable search strategies for storage backends (`strands-ts/src/storage/search/`).

use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;

use super::{Storage, StorageSearchResult};
use crate::error::Result;

/// Shared search-strategy handle.
pub type SearchStrategyRef = Arc<dyn SearchStrategy>;

/// A pluggable way to search stored content (keyword scan, vector similarity, …).
#[async_trait]
pub trait SearchStrategy: Send + Sync {
    /// Search `storage` for `query`, best-first.
    async fn search(&self, storage: &dyn Storage, query: &str) -> Result<Vec<StorageSearchResult>>;

    /// Index one entry on write; strategies that scan on the fly leave this a no-op.
    async fn index(&self, _storage: &dyn Storage, _key: &str, _data: &[u8]) -> Result<()> {
        Ok(())
    }

    /// `true` when the strategy touches the host filesystem itself (`requiresHostFs` in TS).
    fn requires_host_fs(&self) -> bool {
        false
    }
}

/// Split `text` into a set of lowercased word tokens.
pub fn tokenize(text: &str) -> HashSet<String> {
    text.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|s| !s.is_empty())
        .map(|s| s.to_lowercase())
        .collect()
}

/// Lexical relevance: how many distinct tokens of `content` are in `query_tokens`.
pub fn token_overlap_score(query_tokens: &HashSet<String>, content: &str) -> f64 {
    tokenize(content)
        .into_iter()
        .filter(|t| query_tokens.contains(t))
        .count() as f64
}

/// Common English words that carry no retrieval signal.
pub const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being", "have", "has", "had", "do",
    "does", "did", "will", "would", "could", "should", "may", "might", "shall", "can", "need",
    "dare", "ought", "used", "to", "of", "in", "for", "on", "with", "at", "by", "from", "as",
    "into", "through", "during", "before", "after", "above", "below", "between", "out", "off",
    "over", "under", "again", "further", "then", "once", "and", "but", "or", "nor", "not", "so",
    "yet", "both", "either", "neither", "each", "every", "all", "any", "few", "more", "most",
    "other", "some", "such", "no", "only", "own", "same", "than", "too", "very", "just", "because",
    "about", "up", "its", "it", "he", "she", "they", "we", "you", "i", "me", "him", "her", "us",
    "them", "my", "your", "his", "our", "their", "this", "that", "these", "those", "what", "which",
    "who", "whom", "when", "where", "why", "how", "if", "while", "although", "though", "unless",
    "until", "since",
];

/// `true` when `token` is a stop word.
pub fn is_stop_word(token: &str) -> bool {
    STOP_WORDS.contains(&token)
}

/// Keyword search by token overlap over `key + content`. Needs no index; the default everywhere.
#[derive(Clone, Copy, Debug, Default)]
pub struct KeywordSearchStrategy;

#[async_trait]
impl SearchStrategy for KeywordSearchStrategy {
    async fn search(&self, storage: &dyn Storage, query: &str) -> Result<Vec<StorageSearchResult>> {
        let query_tokens = tokenize(query);
        if query_tokens.is_empty() {
            return Ok(Vec::new());
        }
        let mut scored = Vec::new();
        for key in storage.list("").await? {
            let Some(bytes) = storage.read(&key).await? else {
                continue;
            };
            let content = String::from_utf8_lossy(&bytes);
            let score = token_overlap_score(&query_tokens, &format!("{key} {content}"));
            if score > 0.0 {
                scored.push(StorageSearchResult {
                    key,
                    score,
                    data: None,
                });
            }
        }
        scored.sort_by(|a, b| b.score.total_cmp(&a.score));
        Ok(scored)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::InMemoryStorage;

    async fn store(entries: &[(&str, &str)]) -> InMemoryStorage {
        let s = InMemoryStorage::new();
        for (k, v) in entries {
            s.write(k, v.as_bytes()).await.unwrap();
        }
        s
    }

    #[tokio::test]
    async fn returns_matching_entries_scored_by_token_overlap() {
        let s = store(&[
            ("a", "dark mode toggle in settings"),
            ("b", "unrelated content here"),
        ])
        .await;
        let hits = KeywordSearchStrategy.search(&s, "dark mode").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "a");
        assert_eq!(hits[0].score, 2.0);
    }

    #[tokio::test]
    async fn returns_empty_array_for_empty_query() {
        let s = store(&[("a", "anything")]).await;
        assert!(KeywordSearchStrategy.search(&s, "").await.unwrap().is_empty());
    }

    #[tokio::test]
    async fn returns_empty_array_for_whitespace_only_query() {
        let s = store(&[("a", "anything")]).await;
        assert!(KeywordSearchStrategy
            .search(&s, "   ")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn matches_case_insensitively() {
        let s = store(&[("a", "Dark Mode Toggle")]).await;
        let hits = KeywordSearchStrategy.search(&s, "DARK mode").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].score, 2.0);
    }

    #[tokio::test]
    async fn includes_the_key_in_the_scoring_text() {
        let s = store(&[("preferences/theme", "value")]).await;
        let hits = KeywordSearchStrategy.search(&s, "theme").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].key, "preferences/theme");
    }

    #[tokio::test]
    async fn ranks_results_by_score_descending() {
        let s = store(&[
            ("low", "alpha only"),
            ("high", "alpha beta gamma"),
            ("mid", "alpha beta"),
        ])
        .await;
        let hits = KeywordSearchStrategy
            .search(&s, "alpha beta gamma")
            .await
            .unwrap();
        let keys: Vec<&str> = hits.iter().map(|h| h.key.as_str()).collect();
        assert_eq!(keys, vec!["high", "mid", "low"]);
    }

    #[tokio::test]
    async fn returns_empty_array_when_no_content_matches() {
        let s = store(&[("a", "completely different")]).await;
        assert!(KeywordSearchStrategy
            .search(&s, "zzz")
            .await
            .unwrap()
            .is_empty());
    }

    #[test]
    fn tokenize_splits_on_non_word_characters() {
        let t = tokenize("Hello, world_1! café");
        assert!(t.contains("hello") && t.contains("world_1") && t.contains("café"));
        assert_eq!(t.len(), 3);
    }

    #[test]
    fn stop_words_are_recognized() {
        assert!(is_stop_word("the"));
        assert!(!is_stop_word("toggle"));
    }
}
