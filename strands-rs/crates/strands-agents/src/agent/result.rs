//! `AgentResult` (`strands-ts/src/types/agent.ts`).

use std::fmt;

use serde::Serialize;

use super::InvocationState;
use crate::types::{ContentBlock, JsonValue, Message, StopReason};

/// Result of one agent invocation.
#[derive(Clone, Debug)]
pub struct AgentResult {
    /// Why the loop stopped.
    pub stop_reason: StopReason,
    /// The final message in the conversation.
    pub last_message: Message,
    /// The invocation's shared state (same handle every event carried).
    pub invocation_state: InvocationState,
    /// Parsed structured output, when requested.
    pub structured_output: Option<JsonValue>,
}

impl AgentResult {
    /// Wire form: `{ type, stopReason, lastMessage, structuredOutput? }`.
    pub fn to_json(&self) -> JsonValue {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Wire<'a> {
            r#type: &'static str,
            stop_reason: &'a StopReason,
            last_message: &'a Message,
            #[serde(skip_serializing_if = "Option::is_none")]
            structured_output: &'a Option<JsonValue>,
        }
        serde_json::to_value(Wire {
            r#type: "agentResult",
            stop_reason: &self.stop_reason,
            last_message: &self.last_message,
            structured_output: &self.structured_output,
        })
        .expect("AgentResult is serializable")
    }
}

impl PartialEq for AgentResult {
    fn eq(&self, other: &Self) -> bool {
        self.stop_reason == other.stop_reason
            && self.last_message == other.last_message
            && self.structured_output == other.structured_output
    }
}

/// Structured output as JSON if present; otherwise the last message's text and reasoning
/// blocks joined by newlines (`AgentResult.toString()` in TS).
impl fmt::Display for AgentResult {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
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
    fn display_prefers_structured_output() {
        let r = result(vec![ContentBlock::text("hi")], Some(serde_json::json!({"k": 1})));
        assert_eq!(r.to_string(), "{\"k\":1}");
        assert_eq!(r.to_json()["type"], "agentResult");
        assert_eq!(r.to_json()["structuredOutput"]["k"], 1);
    }
}
