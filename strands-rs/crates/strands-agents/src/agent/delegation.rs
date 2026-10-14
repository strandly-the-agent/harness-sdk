//! `AgentDelegation` (`strands-ts/src/agent/agent-delegation.ts`): enforces delegation semantics
//! for tool routing.
//!
//! When a tool is marked as a delegation tool, this plugin ensures:
//! 1. the delegation tool is the only tool called in the turn (single-call constraint);
//! 2. the agent loop exits immediately after a successful delegation (via `end_turn`);
//! 3. the delegation tool's content blocks become the final assistant message.
//!
//! Deviation from TS: TS resolves delegation tools as `AgentAsTool` instances with
//! `delegate: true` (`agent.as_tool({ delegate: true })`), which Rust does not have yet. Until it
//! lands, delegation tools are named explicitly on the plugin
//! ([`AgentDelegation::new`] / [`AgentDelegation::delegate_tool`]); the name set is the only
//! difference — every other behaviour matches TS. A default (empty) plugin is a no-op.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::{
    hook_order, AfterToolCallEvent, AfterToolsEvent, BeforeModelCallEvent, BeforeToolsEvent, Cancel,
};
use crate::plugins::Plugin;
use crate::types::{ContentBlock, ToolResultContent, ToolResultStatus};

/// Plugin name; `AgentBuilder::build` skips the default registration when it is already taken.
pub(crate) const PLUGIN_NAME: &str = "strands:agent-delegation";

/// Cancellation message for a delegation tool called alongside other tools (verbatim from TS).
pub const DELEGATION_SINGLE_CALL_CANCEL: &str = "This tool call was not executed. A delegation tool must be the only tool called in a turn. Retry with a single delegation tool call or use only non-delegation tools.";

/// Per-agent state tracked across the delegation lifecycle within a single invocation.
#[derive(Default)]
struct DelegationState {
    /// Number of tool use blocks in the current batch (from `BeforeToolsEvent`).
    tool_use_count: usize,
    /// Tool use id of the delegation tool that succeeded (set by `AfterToolCallEvent`).
    tool_use_id: Option<String>,
}

/// Plugin that enforces delegation semantics for tool routing.
///
/// Registered on every agent by [`AgentBuilder::build`](crate::agent::AgentBuilder::build); a
/// no-op when no delegation tool fires.
///
/// ```ignore
/// let agent = Agent::builder()
///     .plugin(AgentDelegation::new(["handoff_to_specialist"]))
///     .build()
///     .await?;
/// ```
#[derive(Clone, Default)]
pub struct AgentDelegation {
    delegate_tools: HashSet<String>,
    state: Arc<Mutex<HashMap<String, DelegationState>>>,
}

impl AgentDelegation {
    /// Plugin treating `tool_names` as delegation tools.
    pub fn new<I, S>(tool_names: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            delegate_tools: tool_names.into_iter().map(Into::into).collect(),
            state: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Adds one more delegation tool name.
    pub fn delegate_tool(mut self, tool_name: impl Into<String>) -> Self {
        self.delegate_tools.insert(tool_name.into());
        self
    }

    /// Whether `tool_name` is a delegation tool.
    pub fn is_delegation_tool(&self, tool_name: &str) -> bool {
        self.delegate_tools.contains(tool_name)
    }
}

/// `ToolResultContent` → assistant `ContentBlock`s; JSON is serialized to text (TS parity).
fn to_content_blocks(content: &[ToolResultContent]) -> Vec<ContentBlock> {
    content
        .iter()
        .map(|block| match block {
            ToolResultContent::Text(text) => ContentBlock::Text(text.clone()),
            ToolResultContent::Json(json) => ContentBlock::Text(json.to_string()),
            ToolResultContent::Image(image) => ContentBlock::Image(image.clone()),
            ToolResultContent::Video(video) => ContentBlock::Video(video.clone()),
            ToolResultContent::Document(doc) => ContentBlock::Document(doc.clone()),
        })
        .collect()
}

/// TS `isBlank`: no blocks, or every block is whitespace-only text.
fn is_blank(blocks: &[ContentBlock]) -> bool {
    blocks
        .iter()
        .all(|block| matches!(block, ContentBlock::Text(text) if text.trim().is_empty()))
}

#[async_trait]
impl Plugin for AgentDelegation {
    fn name(&self) -> &str {
        PLUGIN_NAME
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        if self.delegate_tools.is_empty() {
            return Ok(());
        }
        let hooks = agent.hooks();

        // Single-call constraint, plus per-batch state.
        let plugin = self.clone();
        let _ = hooks.add_sync_callback::<BeforeToolsEvent, _>(move |event| {
            let tool_uses: Vec<&str> = event
                .message
                .content
                .iter()
                .filter_map(|block| match block {
                    ContentBlock::ToolUse(tool_use) => Some(tool_use.name.as_str()),
                    _ => None,
                })
                .collect();
            let has_delegation = tool_uses.iter().any(|name| plugin.is_delegation_tool(name));
            plugin.state.lock().insert(
                event.agent.id().to_owned(),
                DelegationState {
                    tool_use_count: tool_uses.len(),
                    tool_use_id: None,
                },
            );
            if has_delegation && tool_uses.len() > 1 {
                event.cancel = Cancel::Reason(DELEGATION_SINGLE_CALL_CANCEL.to_owned());
            }
            Ok(())
        });

        // Mark the delegation tool use that succeeded.
        let plugin = self.clone();
        let _ = hooks.add_sync_callback::<AfterToolCallEvent, _>(move |event| {
            let mut state = plugin.state.lock();
            let Some(entry) = state.get_mut(event.agent.id()) else {
                return Ok(());
            };
            let delegated = plugin.is_delegation_tool(&event.tool_use.name);
            if !delegated || event.error.is_some() || event.result.status != ToolResultStatus::Success {
                // Clear a stale mark if a retry swapped to a non-delegation tool.
                if entry.tool_use_id.as_deref() == Some(event.tool_use.tool_use_id.as_str()) {
                    entry.tool_use_id = None;
                }
                return Ok(());
            }
            if entry.tool_use_count == 1 {
                entry.tool_use_id = Some(event.tool_use.tool_use_id.clone());
            }
            Ok(())
        });

        // End the turn with the delegate's content as the final assistant message.
        let plugin = self.clone();
        let _ =
            hooks.add_sync_callback_with_order::<AfterToolsEvent, _>(hook_order::SDK_LAST, move |event| {
                let tool_use_id = {
                    let mut state = plugin.state.lock();
                    match state.remove(event.agent.id()) {
                        Some(entry) => entry.tool_use_id,
                        None => None,
                    }
                };
                let Some(tool_use_id) = tool_use_id else {
                    return Ok(());
                };
                let blocks = event
                    .message
                    .content
                    .iter()
                    .find_map(|block| match block {
                        ContentBlock::ToolResult(result) if result.tool_use_id == tool_use_id => {
                            Some(to_content_blocks(&result.content))
                        }
                        _ => None,
                    })
                    .unwrap_or_default();
                if is_blank(&blocks) {
                    tracing::debug!(
                        tool_use_id = %tool_use_id,
                        "delegation produced blank content, skipping delegation"
                    );
                    return Ok(());
                }
                event.end_turn = crate::hooks::EndTurn::Blocks(blocks);
                Ok(())
            });

        // If the loop continues past a delegation batch, the next model call invalidates the flag.
        let plugin = self.clone();
        let _ = hooks.add_sync_callback::<BeforeModelCallEvent, _>(move |event| {
            plugin.state.lock().remove(event.agent.id());
            Ok(())
        });

        Ok(())
    }
}

#[cfg(all(test, feature = "test-support"))]
mod tests {
    use super::*;
    use crate::agent::Agent;
    use crate::test_support::{MockMessageModel, MockResponse};
    use crate::tools::FunctionTool;
    use crate::types::{Message, Role, StopReason, ToolUseBlock};

    fn delegate_tool(name: &'static str, reply: &'static str) -> FunctionTool {
        FunctionTool::builder(name)
            .description(format!("delegate to {name}"))
            .input_schema(serde_json::json!({ "type": "object", "properties": {} }))
            .handler(move |_input: serde_json::Value, _ctx| async move { Ok(reply.to_owned()) })
    }

    fn tool_use_message(uses: &[(&str, &str)]) -> Message {
        Message::new(
            Role::Assistant,
            uses.iter()
                .map(|(id, name)| ContentBlock::ToolUse(ToolUseBlock::new(*name, *id, serde_json::json!({}))))
                .collect(),
        )
    }

    #[tokio::test]
    async fn delegation_result_ends_the_turn_as_the_assistant_message() {
        let model = MockMessageModel::shared(vec![MockResponse::with_stop_reason(
            tool_use_message(&[("t1", "handoff")]),
            StopReason::ToolUse,
        )]);
        let agent = Agent::builder()
            .model(model.clone())
            .tool(delegate_tool("handoff", "specialist says hi"))
            .plugin(AgentDelegation::new(["handoff"]))
            .build()
            .await
            .unwrap();

        let result = agent.invoke("go").await.unwrap();

        assert_eq!(
            result.last_message.content,
            vec![ContentBlock::Text("specialist says hi".into())]
        );
        assert_eq!(model.call_count(), 1, "loop exits immediately after delegation");
    }

    #[tokio::test]
    async fn non_delegation_tool_keeps_looping() {
        let model = MockMessageModel::shared(vec![
            MockResponse::with_stop_reason(tool_use_message(&[("t1", "plain")]), StopReason::ToolUse),
            MockResponse::message(Message::assistant("done")),
        ]);
        let agent = Agent::builder()
            .model(model.clone())
            .tool(delegate_tool("plain", "tool output"))
            .plugin(AgentDelegation::new(["handoff"]))
            .build()
            .await
            .unwrap();

        let result = agent.invoke("go").await.unwrap();

        assert_eq!(
            result.last_message.content,
            vec![ContentBlock::Text("done".into())]
        );
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn cancels_the_batch_when_a_delegation_tool_is_called_alongside_another_tool() {
        let model = MockMessageModel::shared(vec![
            MockResponse::with_stop_reason(
                tool_use_message(&[("t1", "handoff"), ("t2", "plain")]),
                StopReason::ToolUse,
            ),
            MockResponse::message(Message::assistant("retried")),
        ]);
        let agent = Agent::builder()
            .model(model.clone())
            .tool(delegate_tool("handoff", "specialist says hi"))
            .tool(delegate_tool("plain", "tool output"))
            .plugin(AgentDelegation::new(["handoff"]))
            .build()
            .await
            .unwrap();

        let result = agent.invoke("go").await.unwrap();

        let results = agent
            .messages()
            .into_iter()
            .flat_map(|m| m.content)
            .collect::<Vec<_>>();
        let texts: Vec<String> = results
            .iter()
            .filter_map(|block| match block {
                ContentBlock::ToolResult(r) => Some(r.content.clone()),
                _ => None,
            })
            .flatten()
            .filter_map(|c| match c {
                ToolResultContent::Text(t) => Some(t),
                _ => None,
            })
            .collect();
        assert!(
            texts.iter().all(|t| t == DELEGATION_SINGLE_CALL_CANCEL),
            "both tool calls are cancelled with the verbatim message, got {texts:?}"
        );
        assert_eq!(
            result.last_message.content,
            vec![ContentBlock::Text("retried".into())]
        );
        assert_eq!(model.call_count(), 2, "the loop continues so the model can retry");
    }

    #[tokio::test]
    async fn blank_delegation_content_does_not_end_the_turn() {
        let model = MockMessageModel::shared(vec![
            MockResponse::with_stop_reason(tool_use_message(&[("t1", "handoff")]), StopReason::ToolUse),
            MockResponse::message(Message::assistant("continued")),
        ]);
        let agent = Agent::builder()
            .model(model.clone())
            .tool(delegate_tool("handoff", "   "))
            .plugin(AgentDelegation::new(["handoff"]))
            .build()
            .await
            .unwrap();

        let result = agent.invoke("go").await.unwrap();

        assert_eq!(
            result.last_message.content,
            vec![ContentBlock::Text("continued".into())]
        );
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn default_plugin_is_a_no_op() {
        let model = MockMessageModel::shared(vec![
            MockResponse::with_stop_reason(tool_use_message(&[("t1", "handoff")]), StopReason::ToolUse),
            MockResponse::message(Message::assistant("done")),
        ]);
        let agent = Agent::builder()
            .model(model.clone())
            .tool(delegate_tool("handoff", "specialist says hi"))
            .plugin(AgentDelegation::default())
            .build()
            .await
            .unwrap();

        let result = agent.invoke("go").await.unwrap();

        assert_eq!(
            result.last_message.content,
            vec![ContentBlock::Text("done".into())]
        );
        assert_eq!(model.call_count(), 2);
    }
}
