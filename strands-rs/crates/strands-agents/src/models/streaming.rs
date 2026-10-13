//! Model stream events (`strands-ts/src/models/streaming.ts`). Internally tagged by `type`.

use serde::{Deserialize, Serialize};

use crate::types::{
    base64_bytes, Citation, CitationGeneratedContent, JsonValue, Metrics, Role, StopReason, Usage,
};

/// A raw streaming event from a model provider.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ModelStreamEvent {
    /// A message started.
    #[serde(rename = "modelMessageStartEvent")]
    MessageStart {
        /// Role of the message.
        role: Role,
    },
    /// A content block started.
    #[serde(rename = "modelContentBlockStartEvent")]
    ContentBlockStart {
        /// Start payload (tool use), if any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        start: Option<ContentBlockStart>,
    },
    /// A content block delta.
    #[serde(rename = "modelContentBlockDeltaEvent")]
    ContentBlockDelta {
        /// Delta payload.
        delta: ContentBlockDelta,
    },
    /// A content block finished.
    #[serde(rename = "modelContentBlockStopEvent")]
    ContentBlockStop {},
    /// The message finished.
    #[serde(rename = "modelMessageStopEvent", rename_all = "camelCase")]
    MessageStop {
        /// Why the model stopped.
        stop_reason: StopReason,
        /// Provider-specific extras.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        additional_model_response_fields: Option<JsonValue>,
    },
    /// Usage / metrics metadata.
    #[serde(rename = "modelMetadataEvent")]
    Metadata {
        /// Token usage.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        usage: Option<Usage>,
        /// Latency metrics.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metrics: Option<Metrics>,
        /// Provider trace payload.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        trace: Option<JsonValue>,
    },
    /// Guardrail redaction.
    #[serde(rename = "modelRedactionEvent", rename_all = "camelCase")]
    Redaction {
        /// Input redaction.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        input_redaction: Option<RedactInputContent>,
        /// Output redaction.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output_redaction: Option<RedactOutputContent>,
    },
}

impl ModelStreamEvent {
    /// The TS `type` discriminator string.
    pub fn type_name(&self) -> &'static str {
        match self {
            ModelStreamEvent::MessageStart { .. } => "modelMessageStartEvent",
            ModelStreamEvent::ContentBlockStart { .. } => "modelContentBlockStartEvent",
            ModelStreamEvent::ContentBlockDelta { .. } => "modelContentBlockDeltaEvent",
            ModelStreamEvent::ContentBlockStop {} => "modelContentBlockStopEvent",
            ModelStreamEvent::MessageStop { .. } => "modelMessageStopEvent",
            ModelStreamEvent::Metadata { .. } => "modelMetadataEvent",
            ModelStreamEvent::Redaction { .. } => "modelRedactionEvent",
        }
    }

    /// Convenience constructor for a text delta.
    pub fn text_delta(text: impl Into<String>) -> Self {
        ModelStreamEvent::ContentBlockDelta {
            delta: ContentBlockDelta::Text { text: text.into() },
        }
    }
}

/// Content block start payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlockStart {
    /// A tool-use block is starting.
    #[serde(rename = "toolUseStart", rename_all = "camelCase")]
    ToolUse {
        /// Tool name.
        name: String,
        /// Tool use id.
        tool_use_id: String,
        /// Provider reasoning signature.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reasoning_signature: Option<String>,
    },
}

/// Content block delta payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ContentBlockDelta {
    /// Text.
    #[serde(rename = "textDelta")]
    Text {
        /// Text fragment.
        text: String,
    },
    /// Partial tool-input JSON text.
    #[serde(rename = "toolUseInputDelta")]
    ToolUseInput {
        /// JSON fragment.
        input: String,
    },
    /// Reasoning content.
    #[serde(rename = "reasoningContentDelta", rename_all = "camelCase")]
    ReasoningContent {
        /// Reasoning text fragment (appended).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        /// Signature (replaces).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        signature: Option<String>,
        /// Redacted bytes (replaces).
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            with = "base64_bytes::option"
        )]
        redacted_content: Option<Vec<u8>>,
    },
    /// Citations.
    #[serde(rename = "citationsDelta")]
    Citations {
        /// Citations.
        citations: Vec<Citation>,
        /// Generated text they annotate.
        content: Vec<CitationGeneratedContent>,
    },
}

/// Input redaction payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactInputContent {
    /// Replacement for the user's input.
    pub replace_content: String,
}

/// Output redaction payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RedactOutputContent {
    /// The redacted output.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redacted_content: Option<String>,
    /// Replacement for the model's output.
    pub replace_content: String,
}

/// Add `source` into `target`; cache fields are only added when present on `source`.
pub fn accumulate_usage(target: &mut Usage, source: &Usage) {
    target.input_tokens += source.input_tokens;
    target.output_tokens += source.output_tokens;
    target.total_tokens += source.total_tokens;
    if let Some(r) = source.cache_read_input_tokens {
        target.cache_read_input_tokens = Some(target.cache_read_input_tokens.unwrap_or(0) + r);
    }
    if let Some(w) = source.cache_write_input_tokens {
        target.cache_write_input_tokens = Some(target.cache_write_input_tokens.unwrap_or(0) + w);
    }
}

/// Prompt tokens including cache reads/writes when the provider reports them separately.
pub fn total_prompt_tokens(usage: &Usage) -> u64 {
    if usage.input_tokens + usage.output_tokens == usage.total_tokens {
        usage.input_tokens
    } else {
        usage.input_tokens
            + usage.cache_read_input_tokens.unwrap_or(0)
            + usage.cache_write_input_tokens.unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn events_are_internally_tagged() {
        let ev = ModelStreamEvent::MessageStop {
            stop_reason: StopReason::EndTurn,
            additional_model_response_fields: None,
        };
        assert_eq!(
            serde_json::to_value(&ev).unwrap(),
            json!({"type": "modelMessageStopEvent", "stopReason": "endTurn"})
        );
        let start = ModelStreamEvent::ContentBlockStart {
            start: Some(ContentBlockStart::ToolUse {
                name: "t".into(),
                tool_use_id: "id".into(),
                reasoning_signature: None,
            }),
        };
        assert_eq!(
            serde_json::to_value(&start).unwrap(),
            json!({"type": "modelContentBlockStartEvent", "start": {"type": "toolUseStart", "name": "t", "toolUseId": "id"}})
        );
        assert_eq!(
            serde_json::to_value(ModelStreamEvent::text_delta("x")).unwrap(),
            json!({"type": "modelContentBlockDeltaEvent", "delta": {"type": "textDelta", "text": "x"}})
        );
        assert!(serde_json::from_value::<ModelStreamEvent>(json!({"type": "bogus"})).is_err());
    }

    #[test]
    fn accumulate_usage_adds_cache_fields_only_when_present() {
        let mut target = Usage {
            input_tokens: 1,
            output_tokens: 2,
            total_tokens: 3,
            ..Default::default()
        };
        accumulate_usage(
            &mut target,
            &Usage {
                input_tokens: 1,
                output_tokens: 1,
                total_tokens: 2,
                ..Default::default()
            },
        );
        assert_eq!(target.cache_read_input_tokens, None);
        accumulate_usage(
            &mut target,
            &Usage {
                cache_read_input_tokens: Some(5),
                ..Default::default()
            },
        );
        assert_eq!(
            (
                target.input_tokens,
                target.total_tokens,
                target.cache_read_input_tokens
            ),
            (2, 5, Some(5))
        );
    }

    #[test]
    fn total_prompt_tokens_includes_cache_when_totals_differ() {
        let plain = Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            ..Default::default()
        };
        assert_eq!(total_prompt_tokens(&plain), 10);
        let cached = Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 100,
            cache_read_input_tokens: Some(80),
            cache_write_input_tokens: Some(5),
        };
        assert_eq!(total_prompt_tokens(&cached), 95);
    }
}
