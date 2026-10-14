//! `ModelExtractor` (`strands-ts/src/memory/extraction/model-extractor.ts`): asks a model to
//! distill a transcript into discrete facts.

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::memory::types::{ExtractionResult, Extractor, Metadata};
use crate::models::{Model, StreamOptions};
use crate::types::{ContentBlock, Message, Role, SystemPrompt};

/// Default instruction guiding the model to emit discrete, durable facts as a JSON array.
pub const DEFAULT_SYSTEM_PROMPT: &str = "You extract durable facts worth remembering across future conversations from a transcript.\n\nReturn ONLY a JSON array of objects, each: {\"content\": string}. Each object is one discrete, self-contained fact (a preference, decision, or stable detail about the user or task). Do not include transient chit-chat, questions, or anything already obvious. If there is nothing worth remembering, return [].";

/// An [`Extractor`] that calls a model to distill messages into facts.
pub struct ModelExtractor {
    model: Arc<dyn Model>,
    system_prompt: String,
}

impl ModelExtractor {
    /// Extractor over `model` with the default fact-extraction prompt.
    pub fn new(model: Arc<dyn Model>) -> Self {
        Self {
            model,
            system_prompt: DEFAULT_SYSTEM_PROMPT.to_owned(),
        }
    }

    /// Override the system prompt steering what counts as a fact.
    pub fn system_prompt(mut self, prompt: impl Into<String>) -> Self {
        self.system_prompt = prompt.into();
        self
    }
}

#[async_trait]
impl Extractor for ModelExtractor {
    async fn extract(&self, messages: &[Message]) -> Result<Vec<ExtractionResult>> {
        if messages.is_empty() {
            return Ok(Vec::new());
        }
        let transcript = messages.iter().map(render_message).collect::<Vec<_>>().join("\n");
        let prompt = vec![Message::new(
            Role::User,
            vec![ContentBlock::Text(format!(
                "Extract facts from the following transcript:\n\n{transcript}"
            ))],
        )];
        let options = StreamOptions {
            system_prompt: Some(SystemPrompt::from(self.system_prompt.clone())),
            ..Default::default()
        };
        let result = self.model.aggregate(&prompt, &options).await?;
        let text: String = result
            .message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(text) => Some(text.as_str()),
                _ => None,
            })
            .collect::<Vec<_>>()
            .join("");
        Ok(parse_entries(text.trim()))
    }
}

fn role_label(role: Role) -> &'static str {
    match role {
        Role::User => "user",
        Role::Assistant => "assistant",
    }
}

/// Renders one message as `role: text`, joining its text blocks.
fn render_message(message: &Message) -> String {
    let text = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) if !text.is_empty() => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n");
    format!("{}: {}", role_label(message.role), text)
}

/// Parses the model's response. Tolerates prose or a fenced block around the array; anything
/// unparseable yields no entries (extraction must never fail the turn).
fn parse_entries(text: &str) -> Vec<ExtractionResult> {
    let Some(json) = extract_json_array(text) else {
        tracing::warn!(
            "ModelExtractor: no JSON array in model output, skipping | text_len=<{}>",
            text.len()
        );
        return Vec::new();
    };
    let Ok(parsed) = serde_json::from_str::<serde_json::Value>(json) else {
        tracing::warn!("ModelExtractor: failed to parse output");
        return Vec::new();
    };
    let Some(items) = parsed.as_array() else {
        return Vec::new();
    };
    items
        .iter()
        .filter_map(|item| {
            let content = item.get("content")?.as_str()?.trim();
            if content.is_empty() {
                return None;
            }
            let metadata = item.get("metadata").and_then(|m| m.as_object()).map(|m| {
                m.iter()
                    .map(|(k, v)| (k.clone(), v.clone()))
                    .collect::<Metadata>()
            });
            Some(ExtractionResult {
                content: content.to_owned(),
                metadata,
            })
        })
        .collect()
}

/// The substring from the first `[` to the last `]`, or `None`.
fn extract_json_array(text: &str) -> Option<&str> {
    let start = text.find('[')?;
    let end = text.rfind(']')?;
    if end < start {
        return None;
    }
    Some(&text[start..=end])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{MockMessageModel, MockResponse};

    fn extractor_returning(text: &str) -> ModelExtractor {
        let model = MockMessageModel::new(vec![MockResponse::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::Text(text.to_owned())],
        ))]);
        ModelExtractor::new(Arc::new(model))
    }

    fn user(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::Text(text.to_owned())])
    }

    #[tokio::test]
    async fn extracts_facts_from_a_json_array() {
        let extractor = extractor_returning(r#"[{"content": "Likes dark mode"}, {"content": "Uses vim"}]"#);
        let facts = extractor.extract(&[user("i like dark mode")]).await.unwrap();
        assert_eq!(
            facts,
            vec![
                ExtractionResult {
                    content: "Likes dark mode".into(),
                    metadata: None
                },
                ExtractionResult {
                    content: "Uses vim".into(),
                    metadata: None
                },
            ]
        );
    }

    #[tokio::test]
    async fn tolerates_prose_and_fences_around_the_array() {
        let extractor = extractor_returning(
            "Sure!\n```json\n[{\"content\": \"  Prefers tabs \"}]\n```\nHope that helps.",
        );
        let facts = extractor.extract(&[user("tabs please")]).await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].content, "Prefers tabs");
    }

    #[tokio::test]
    async fn keeps_entry_metadata_and_drops_malformed_items() {
        let extractor = extractor_returning(
            r#"[{"content": "a", "metadata": {"k": 1}}, {"content": ""}, {"nope": true}, 7]"#,
        );
        let facts = extractor.extract(&[user("x")]).await.unwrap();
        assert_eq!(facts.len(), 1);
        assert_eq!(facts[0].metadata.as_ref().unwrap()["k"], serde_json::json!(1));
    }

    #[tokio::test]
    async fn unparseable_output_yields_no_entries() {
        for text in ["no array here", "[not json", "{\"content\": \"a\"}"] {
            let facts = extractor_returning(text).extract(&[user("x")]).await.unwrap();
            assert!(facts.is_empty(), "text={text}");
        }
    }

    #[tokio::test]
    async fn no_messages_skips_the_model_call() {
        let extractor = extractor_returning("[]");
        assert!(extractor.extract(&[]).await.unwrap().is_empty());
    }

    #[test]
    fn renders_messages_as_role_prefixed_text() {
        assert_eq!(render_message(&user("hi")), "user: hi");
    }
}
