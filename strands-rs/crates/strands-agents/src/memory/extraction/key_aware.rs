//! The key-aware extractor (`vended-memory-stores/file-memory-store/store.ts`): a
//! [`ModelExtractor`] whose system prompt lists the topics already on disk, so new facts land
//! under an existing heading instead of spawning a near-duplicate file.

use std::sync::Arc;

use crate::error::Result;
use crate::memory::types::ExtractorRef;
use crate::memory::ModelExtractor;
use crate::models::Model;
use crate::storage::StorageRef;

/// Default framing of the extraction prompt, replaced by a custom `system_prompt`.
pub const DEFAULT_EXTRACTION_GUIDANCE: &str =
    "You extract durable facts worth remembering across future conversations from a transcript.";

/// The output contract, always appended after the framing so an override cannot break parsing.
pub const EXTRACTION_CONTRACT: &str = "Return ONLY a JSON array of objects: {\"content\": string}.\n\nGroup related facts into a single entry. The first line is a markdown heading (e.g. \"# User preferences\", \"# Project setup\", \"# Team conventions\"). Put each fact on its own line below the heading.\n\nIf there is nothing worth remembering, return [].";

/// Compose the key-aware system prompt from the topics already in `storage`.
pub async fn key_aware_system_prompt(storage: &StorageRef, system_prompt: Option<&str>) -> Result<String> {
    let headings = existing_headings(storage).await?;
    Ok(compose_prompt(&headings, system_prompt))
}

/// A [`ModelExtractor`] over `model` prompted with the topics already in `storage`.
pub async fn create_key_aware_extractor(
    storage: &StorageRef,
    model: Arc<dyn Model>,
    system_prompt: Option<&str>,
) -> Result<ExtractorRef> {
    let prompt = key_aware_system_prompt(storage, system_prompt).await?;
    Ok(Arc::new(ModelExtractor::new(model).system_prompt(prompt)))
}

async fn existing_headings(storage: &StorageRef) -> Result<Vec<String>> {
    let keys = storage.list("").await?;
    Ok(keys.iter().map(|key| heading_of(key)).collect())
}

/// `memory/notes/user-preferences.md` → `user preferences`, the inverse of the store's slugify.
fn heading_of(key: &str) -> String {
    let filename = key.rsplit('/').next().unwrap_or(key);
    let stem = filename.strip_suffix(".md").unwrap_or(filename);
    stem.replace('-', " ")
}

fn compose_prompt(headings: &[String], system_prompt: Option<&str>) -> String {
    let framing = system_prompt.unwrap_or(DEFAULT_EXTRACTION_GUIDANCE);
    let mut composed = format!("{framing}\n\n{EXTRACTION_CONTRACT}");
    if !headings.is_empty() {
        composed.push_str(&format!(
            "\n\nExisting topics: {}. Reuse an existing topic heading when new facts belong to it.",
            headings.join(", ")
        ));
    }
    composed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::{InMemoryStorage, Storage};

    async fn storage_with(keys: &[&str]) -> StorageRef {
        let storage = InMemoryStorage::new();
        for key in keys {
            storage.write(key, b"# body").await.unwrap();
        }
        Arc::new(storage)
    }

    #[tokio::test]
    async fn includes_existing_topic_headings_in_the_system_prompt() {
        let storage = storage_with(&["user-preferences.md", "project-setup.md"]).await;
        let prompt = key_aware_system_prompt(&storage, None).await.unwrap();
        assert!(prompt.contains("Existing topics: "), "{prompt}");
        assert!(prompt.contains("user preferences"), "{prompt}");
        assert!(prompt.contains("project setup"), "{prompt}");
        assert!(prompt.contains("Reuse an existing topic heading when new facts belong to it."));
    }

    #[tokio::test]
    async fn does_not_include_the_headings_line_when_there_are_none() {
        let storage = storage_with(&[]).await;
        let prompt = key_aware_system_prompt(&storage, None).await.unwrap();
        assert!(!prompt.contains("Existing topics"));
        assert_eq!(
            prompt,
            format!("{DEFAULT_EXTRACTION_GUIDANCE}\n\n{EXTRACTION_CONTRACT}")
        );
    }

    #[tokio::test]
    async fn builds_the_system_prompt_on_the_key_aware_heading_instruction() {
        let storage = storage_with(&[]).await;
        let prompt = key_aware_system_prompt(&storage, None).await.unwrap();
        assert!(prompt.starts_with(DEFAULT_EXTRACTION_GUIDANCE));
        assert!(prompt.contains("The first line is a markdown heading"));
        assert!(prompt.contains("Return ONLY a JSON array of objects"));
    }

    #[tokio::test]
    async fn overrides_the_framing_with_a_custom_system_prompt_but_keeps_the_output_contract() {
        let storage = storage_with(&["a-b.md"]).await;
        let prompt = key_aware_system_prompt(&storage, Some("Only remember pets."))
            .await
            .unwrap();
        assert!(prompt.starts_with("Only remember pets."));
        assert!(!prompt.contains(DEFAULT_EXTRACTION_GUIDANCE));
        assert!(prompt.contains(EXTRACTION_CONTRACT));
        assert!(prompt.contains("Existing topics: a b."));
    }

    #[test]
    fn heading_strips_the_directory_and_extension_and_undoes_the_slug() {
        assert_eq!(heading_of("memory/notes/user-preferences.md"), "user preferences");
        assert_eq!(heading_of("team-conventions"), "team conventions");
    }
}
