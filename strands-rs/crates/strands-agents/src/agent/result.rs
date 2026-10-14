//! `AgentResult` (`strands-ts/src/types/agent.ts`).

use std::fmt;

use serde::Serialize;

use super::InvocationState;
use crate::interrupt::Interrupt;
use crate::telemetry::{AgentMetrics, AgentTrace};
use crate::types::{ContentBlock, JsonValue, Message, StopReason};

/// Result of one agent invocation.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "type", rename = "agentResult", rename_all = "camelCase")]
#[non_exhaustive]
pub struct AgentResult {
    /// Why the loop stopped.
    pub stop_reason: StopReason,
    /// The final message in the conversation.
    pub last_message: Message,
    /// The invocation's shared state (same handle every event carried).
    #[serde(skip)]
    pub invocation_state: InvocationState,
    /// Parsed structured output, when requested.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub structured_output: Option<JsonValue>,
    /// Metrics collected during the invocation.
    pub metrics: AgentMetrics,
    /// Local trace tree collected during the invocation.
    pub traces: Vec<AgentTrace>,
    /// Unanswered interrupts; non-empty exactly when `stop_reason` is
    /// [`StopReason::Interrupt`](crate::types::StopReason::Interrupt).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub interrupts: Vec<Interrupt>,
    /// The cycle boundary the loop paused at, when `stop_reason` is `checkpoint`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub checkpoint: Option<super::Checkpoint>,
}

impl AgentResult {
    /// A result with no structured output.
    pub fn new(stop_reason: StopReason, last_message: Message, invocation_state: InvocationState) -> Self {
        Self {
            stop_reason,
            last_message,
            invocation_state,
            structured_output: None,
            metrics: AgentMetrics::default(),
            traces: Vec::new(),
            interrupts: Vec::new(),
            checkpoint: None,
        }
    }

    /// Attach parsed structured output.
    #[must_use]
    pub fn with_structured_output(mut self, output: JsonValue) -> Self {
        self.structured_output = Some(output);
        self
    }

    /// Attach telemetry collected during the invocation.
    #[must_use]
    pub fn with_telemetry(mut self, metrics: AgentMetrics, traces: Vec<AgentTrace>) -> Self {
        self.metrics = metrics;
        self.traces = traces;
        self
    }

    /// Total prompt tokens (incl. cached) of the last model call (TS `latestContextSize`).
    pub fn context_size(&self) -> Option<u64> {
        self.metrics.latest_context_size
    }

    /// Latest prompt plus the last output tokens (TS `projectedContextSize`).
    pub fn projected_context_size(&self) -> Option<u64> {
        self.metrics.projected_context_size
    }

    /// Text of the last message (its text blocks concatenated).
    pub fn text(&self) -> String {
        self.last_message.text()
    }

    /// Structured output deserialized into `T`; `None` when the invocation produced none.
    pub fn structured_output_as<T: serde::de::DeserializeOwned>(&self) -> Option<crate::error::Result<T>> {
        let value = self.structured_output.clone()?;
        Some(serde_json::from_value(value).map_err(crate::error::Error::from))
    }

    /// Wire form: `{ type, stopReason, lastMessage, structuredOutput? }`.
    pub fn to_json(&self) -> JsonValue {
        serde_json::to_value(self).expect("AgentResult is serializable")
    }
}

impl PartialEq for AgentResult {
    fn eq(&self, other: &Self) -> bool {
        self.stop_reason == other.stop_reason
            && self.last_message == other.last_message
            && self.structured_output == other.structured_output
            && self.interrupts == other.interrupts
            && self.checkpoint == other.checkpoint
    }
}

/// Structured output as JSON if present; otherwise the last message's text and reasoning
/// blocks joined by newlines (`AgentResult.toString()` in TS).
impl fmt::Display for AgentResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if !self.interrupts.is_empty() {
            let json = serde_json::to_string(&self.interrupts).unwrap_or_default();
            return f.write_str(&json);
        }
        if let Some(out) = &self.structured_output {
            return f.write_str(&out.to_string());
        }
        let parts: Vec<String> = self
            .last_message
            .content
            .iter()
            .filter_map(|block| match block {
                ContentBlock::Text(t) => Some(t.clone()),
                ContentBlock::Reasoning(r) => r
                    .text
                    .as_ref()
                    .map(|t| format!("💭 Reasoning:\n   {}", t.replace('\n', "\n   "))),
                ContentBlock::Citations(c) => Some(
                    c.content
                        .iter()
                        .map(|c| c.text.as_str())
                        .collect::<Vec<_>>()
                        .join(""),
                ),
                _ => None,
            })
            .collect();
        f.write_str(&parts.join("\n"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ReasoningBlock, Role};

    fn result(content: Vec<ContentBlock>, structured: Option<JsonValue>) -> AgentResult {
        AgentResult {
            stop_reason: StopReason::EndTurn,
            last_message: Message::new(Role::Assistant, content),
            invocation_state: InvocationState::new(),
            structured_output: structured,
            metrics: AgentMetrics::default(),
            traces: Vec::new(),
            interrupts: Vec::new(),
            checkpoint: None,
        }
    }

    #[test]
    fn display_joins_text_and_reasoning() {
        let r = result(
            vec![
                ContentBlock::Reasoning(ReasoningBlock {
                    text: Some("a\nb".into()),
                    ..Default::default()
                }),
                ContentBlock::text("hi"),
            ],
            None,
        );
        assert_eq!(r.to_string(), "💭 Reasoning:\n   a\n   b\nhi");
    }

    #[test]
    fn text_returns_the_last_message_text() {
        let r = result(
            vec![
                ContentBlock::Reasoning(ReasoningBlock {
                    text: Some("thinking".into()),
                    ..Default::default()
                }),
                ContentBlock::text("hi"),
                ContentBlock::text("there"),
            ],
            Some(serde_json::json!({"k": 1})),
        );
        assert_eq!(r.text(), "hithere");
    }

    #[test]
    fn display_prefers_structured_output() {
        let r = result(vec![ContentBlock::text("hi")], Some(serde_json::json!({"k": 1})));
        assert_eq!(r.to_string(), "{\"k\":1}");
        assert_eq!(r.to_json()["type"], "agentResult");
        assert_eq!(r.to_json()["structuredOutput"]["k"], 1);
    }

    #[test]
    fn structured_output_as_deserializes_or_errors() {
        #[derive(serde::Deserialize, PartialEq, Debug)]
        struct Out {
            k: u32,
        }

        let none = result(vec![ContentBlock::text("hi")], None);
        assert!(none.structured_output_as::<Out>().is_none());

        let ok = result(vec![], Some(serde_json::json!({"k": 1})));
        assert_eq!(ok.structured_output_as::<Out>().unwrap().unwrap(), Out { k: 1 });

        let bad = result(vec![], Some(serde_json::json!({"k": "nope"})));
        assert!(bad.structured_output_as::<Out>().unwrap().is_err());
    }
}
