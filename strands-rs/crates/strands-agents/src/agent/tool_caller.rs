//! Direct tool calling: `agent.tool("name")?.call(input)`.
//!
//! Runs a registered tool without model inference and (by default) records the call in the
//! agent's message history, firing a [`MessageAddedEvent`] for each appended message.

use crate::agent::{Agent, InvocationState};
use crate::error::{Error, Result};
use crate::hooks::MessageAddedEvent;
use crate::tools::{ToolContext, ToolRef};
use crate::types::{ContentBlock, JsonValue, Message, Role, ToolResultBlock, ToolUse, ToolUseBlock};

/// Error text when a recording direct tool call is attempted during an invocation.
#[doc(hidden)]
pub const DIRECT_CALL_DURING_INVOCATION_MESSAGE: &str =
    "Direct tool call cannot be made while the agent is in the middle of an invocation. Set \
     record_direct_tool_call: false to allow direct tool calls during agent invocation.";

/// Options for one direct tool call.
///
/// Build from [`CallOptions::default`] and the `with_*` setters.
#[derive(Clone, Debug)]
#[non_exhaustive]
pub struct CallOptions {
    /// Record the call in the agent's message history. Defaults to `true`.
    pub record_direct_tool_call: bool,
    /// Shared state handed to the tool.
    pub invocation_state: InvocationState,
}

impl Default for CallOptions {
    fn default() -> Self {
        Self {
            record_direct_tool_call: true,
            invocation_state: InvocationState::default(),
        }
    }
}

impl CallOptions {
    /// Whether to record the call in the agent's message history.
    #[must_use]
    pub fn with_record_direct_tool_call(mut self, record: bool) -> Self {
        self.record_direct_tool_call = record;
        self
    }

    /// The shared state handed to the tool.
    #[must_use]
    pub fn with_invocation_state(mut self, state: InvocationState) -> Self {
        self.invocation_state = state;
        self
    }
}

/// A handle to one resolved tool, obtained from [`Agent::tool`].
#[derive(Clone)]
#[must_use = "a ToolHandle does nothing until you call call()"]
pub struct ToolHandle {
    agent: Agent,
    tool: ToolRef,
}

impl std::fmt::Debug for ToolHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ToolHandle")
            .field("tool", &self.tool.name())
            .finish()
    }
}

impl Agent {
    /// A handle to a tool by name, resolved fuzzily (exact, `_`→`-`, case-insensitive).
    ///
    /// Returns [`Error::ToolNotFound`] when no tool matches.
    pub fn tool(&self, name: &str) -> Result<ToolHandle> {
        let tool = self.with_tool_registry(|registry| registry.resolve(name))?;
        Ok(ToolHandle {
            agent: self.clone(),
            tool,
        })
    }
}

impl ToolHandle {
    /// The resolved tool's name.
    pub fn name(&self) -> &str {
        self.tool.name()
    }

    /// Run the tool and record the call in the message history.
    pub async fn call(&self, input: JsonValue) -> Result<ToolResultBlock> {
        self.call_with(input, CallOptions::default()).await
    }

    /// [`ToolHandle::call`] with options.
    pub async fn call_with(&self, input: JsonValue, options: CallOptions) -> Result<ToolResultBlock> {
        if options.record_direct_tool_call && self.agent.is_invoking() {
            return Err(Error::ConcurrentInvocation(
                DIRECT_CALL_DURING_INVOCATION_MESSAGE.to_owned(),
            ));
        }

        let input = if input.is_null() {
            JsonValue::Object(serde_json::Map::new())
        } else {
            input
        };
        let tool_use = ToolUse {
            name: self.tool.name().to_owned(),
            tool_use_id: format!("tooluse_{}", uuid::Uuid::new_v4()),
            input,
        };
        let ctx = ToolContext::new(
            tool_use.clone(),
            self.agent.clone(),
            options.invocation_state.clone(),
            self.agent.cancel_token(),
        );

        let result = self.tool.invoke(ctx).await?;

        if options.record_direct_tool_call {
            self.record(&tool_use, &result, &options.invocation_state).await?;
        }
        Ok(result)
    }

    async fn record(
        &self,
        tool_use: &ToolUse,
        result: &ToolResultBlock,
        state: &InvocationState,
    ) -> Result<()> {
        let tool_use_message = Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock {
                name: tool_use.name.clone(),
                tool_use_id: tool_use.tool_use_id.clone(),
                input: tool_use.input.clone(),
                reasoning_signature: None,
            })],
        );
        let result_message = Message::new(Role::User, vec![ContentBlock::ToolResult(result.clone())]);
        let ack_message = Message::assistant(format!("agent.tool.{} was called.", tool_use.name));

        for message in [tool_use_message, result_message, ack_message] {
            self.append(message, state).await?;
        }
        Ok(())
    }

    async fn append(&self, message: Message, state: &InvocationState) -> Result<()> {
        self.agent
            .with_messages(|messages| messages.push(message.clone()));
        let mut event = MessageAddedEvent {
            agent: self.agent.clone(),
            message,
            invocation_state: state.clone(),
        };
        self.agent.hooks().invoke_callbacks(&mut event).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::test_fixtures::test_agent_with;
    use crate::tools::FunctionTool;
    use crate::types::{ToolResultContent, ToolResultStatus};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;

    fn set_invoking(agent: &Agent, value: bool) {
        agent.inner.invoking.store(value, Ordering::SeqCst);
    }

    fn echo_tool(name: &str) -> Arc<FunctionTool> {
        Arc::new(
            FunctionTool::builder(name)
                .description("echo")
                .handler(|input: JsonValue, _ctx| async move { Ok(input) }),
        )
    }

    fn agent_with_tools(tools: Vec<ToolRef>) -> Agent {
        let agent = test_agent_with("tool-caller", None, Vec::new());
        agent
            .with_tool_registry(|registry| registry.add_or_replace(tools))
            .unwrap();
        agent
    }

    fn text_of(result: &ToolResultBlock) -> String {
        result
            .content
            .iter()
            .map(|block| match block {
                ToolResultContent::Text(text) => text.clone(),
                ToolResultContent::Json(value) => value.to_string(),
                _ => String::new(),
            })
            .collect()
    }

    #[tokio::test]
    async fn calls_a_tool_by_name_and_returns_the_result() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        let result = agent
            .tool("calculator")
            .unwrap()
            .call(serde_json::json!({ "a": 5, "b": 3 }))
            .await
            .unwrap();

        assert_eq!(result.status, ToolResultStatus::Success);
        assert_eq!(text_of(&result), r#"{"a":5,"b":3}"#);
    }

    #[tokio::test]
    async fn calls_a_tool_with_empty_input_when_no_input_provided() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        let result = agent
            .tool("calculator")
            .unwrap()
            .call(JsonValue::Null)
            .await
            .unwrap();

        assert_eq!(text_of(&result), "{}");
        let messages = agent.messages();
        let ContentBlock::ToolUse(block) = &messages[0].content[0] else {
            panic!("expected tool use");
        };
        assert_eq!(block.input, serde_json::json!({}));
    }

    #[tokio::test]
    async fn passes_agent_cancel_signal_to_direct_tool_calls() {
        let seen = Arc::new(AtomicBool::new(false));
        let captured = Arc::clone(&seen);
        let tool: ToolRef = Arc::new(FunctionTool::builder("probe").description("p").handler(
            move |_input: JsonValue, ctx: ToolContext| {
                let captured = Arc::clone(&captured);
                async move {
                    captured.store(ctx.cancel.is_cancelled(), Ordering::SeqCst);
                    Ok(JsonValue::from("ok"))
                }
            },
        ));
        let agent = agent_with_tools(vec![tool]);
        set_invoking(&agent, true);
        agent.cancel();

        agent
            .tool("probe")
            .unwrap()
            .call_with(
                JsonValue::Null,
                CallOptions {
                    record_direct_tool_call: false,
                    ..CallOptions::default()
                },
            )
            .await
            .unwrap();
        assert!(seen.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn throws_when_tool_is_not_found() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        let err = agent.tool("missing").unwrap_err();
        assert!(matches!(err, Error::ToolNotFound { .. }), "got {err:?}");
    }

    #[tokio::test]
    async fn resolves_underscore_names_to_hyphenated_tool_names() {
        let agent = agent_with_tools(vec![echo_tool("my-tool")]);
        assert_eq!(agent.tool("my_tool").unwrap().name(), "my-tool");
    }

    #[tokio::test]
    async fn resolves_tool_names_case_insensitively() {
        let agent = agent_with_tools(vec![echo_tool("Calculator")]);
        assert_eq!(agent.tool("calculator").unwrap().name(), "Calculator");
    }

    #[tokio::test]
    async fn prefers_exact_match_over_case_insensitive_match() {
        let agent = agent_with_tools(vec![echo_tool("Calculator"), echo_tool("calculator")]);
        assert_eq!(agent.tool("calculator").unwrap().name(), "calculator");
    }

    #[tokio::test]
    async fn records_tool_call_in_message_history_by_default() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        let result = agent
            .tool("calculator")
            .unwrap()
            .call(serde_json::json!({ "a": 5, "b": 3 }))
            .await
            .unwrap();

        let messages = agent.messages();
        assert_eq!(messages.len(), 3);
        assert_eq!(messages[0].role, Role::Assistant);
        let ContentBlock::ToolUse(block) = &messages[0].content[0] else {
            panic!("expected tool use");
        };
        assert_eq!(block.name, "calculator");
        assert!(block.tool_use_id.starts_with("tooluse_"));
        assert_eq!(block.input, serde_json::json!({ "a": 5, "b": 3 }));

        assert_eq!(messages[1].role, Role::User);
        assert_eq!(
            messages[1].content,
            vec![ContentBlock::ToolResult(result.clone())]
        );

        assert_eq!(messages[2].role, Role::Assistant);
        assert_eq!(
            messages[2].content,
            vec![ContentBlock::Text("agent.tool.calculator was called.".to_owned())]
        );
    }

    #[tokio::test]
    async fn does_not_record_when_record_direct_tool_call_is_false_per_call() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        agent
            .tool("calculator")
            .unwrap()
            .call_with(
                serde_json::json!({ "a": 5 }),
                CallOptions {
                    record_direct_tool_call: false,
                    ..CallOptions::default()
                },
            )
            .await
            .unwrap();

        assert!(agent.messages().is_empty());
    }

    #[tokio::test]
    async fn records_when_explicitly_set_to_true_per_call() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        agent
            .tool("calculator")
            .unwrap()
            .call_with(
                serde_json::json!({ "a": 5 }),
                CallOptions {
                    record_direct_tool_call: true,
                    ..CallOptions::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(agent.messages().len(), 3);
    }

    #[tokio::test]
    async fn records_full_input_without_filtering() {
        let tool: ToolRef = Arc::new(
            FunctionTool::builder("my-tool")
                .description("Tool with strict schema")
                .input_schema(serde_json::json!({
                    "type": "object",
                    "properties": { "allowed": { "type": "string" } }
                }))
                .handler(|input: JsonValue, _ctx| async move { Ok(input) }),
        );
        let agent = agent_with_tools(vec![tool]);

        agent
            .tool("my_tool")
            .unwrap()
            .call(serde_json::json!({ "allowed": "yes", "extra": "also-recorded" }))
            .await
            .unwrap();

        let messages = agent.messages();
        let ContentBlock::ToolUse(block) = &messages[0].content[0] else {
            panic!("expected tool use");
        };
        assert_eq!(
            block.input,
            serde_json::json!({ "allowed": "yes", "extra": "also-recorded" })
        );
    }

    #[tokio::test]
    async fn throws_concurrent_invocation_error_when_agent_is_invoking_and_recording_is_enabled() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        set_invoking(&agent, true);

        let err = agent
            .tool("calculator")
            .unwrap()
            .call(serde_json::json!({}))
            .await
            .unwrap_err();

        assert!(
            matches!(err, Error::ConcurrentInvocation(ref msg) if msg == DIRECT_CALL_DURING_INVOCATION_MESSAGE)
        );
    }

    #[tokio::test]
    async fn allows_direct_tool_call_during_invocation_when_record_direct_tool_call_is_false() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        set_invoking(&agent, true);

        let result = agent
            .tool("calculator")
            .unwrap()
            .call_with(
                serde_json::json!({ "a": 1 }),
                CallOptions {
                    record_direct_tool_call: false,
                    ..CallOptions::default()
                },
            )
            .await
            .unwrap();

        assert_eq!(result.status, ToolResultStatus::Success);
        assert!(agent.messages().is_empty());
    }

    #[tokio::test]
    async fn is_invoking_is_false_on_a_fresh_agent() {
        let agent = agent_with_tools(Vec::new());
        assert!(!agent.is_invoking());
    }

    #[tokio::test]
    async fn propagates_errors_when_tool_throws() {
        struct Failing;

        #[async_trait::async_trait]
        impl crate::tools::Tool for Failing {
            fn name(&self) -> &str {
                "failing"
            }
            fn description(&self) -> &str {
                "fails"
            }
            fn spec(&self) -> &crate::types::ToolSpec {
                static SPEC: std::sync::OnceLock<crate::types::ToolSpec> = std::sync::OnceLock::new();
                SPEC.get_or_init(|| crate::types::ToolSpec::new("failing", "fails"))
            }
            async fn invoke(&self, _ctx: ToolContext) -> Result<ToolResultBlock> {
                Err(Error::Other("Tool failed".into()))
            }
        }

        let agent = agent_with_tools(vec![Arc::new(Failing)]);
        let err = agent
            .tool("failing")
            .unwrap()
            .call(JsonValue::Null)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("Tool failed"), "got {err}");
        assert!(agent.messages().is_empty());
    }

    #[tokio::test]
    async fn generates_unique_tool_use_ids() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        let handle = agent.tool("calculator").unwrap();
        handle.call(serde_json::json!({})).await.unwrap();
        handle.call(serde_json::json!({})).await.unwrap();

        let messages = agent.messages();
        let ids: Vec<String> = messages
            .iter()
            .filter_map(|m| match m.content.first() {
                Some(ContentBlock::ToolUse(block)) => Some(block.tool_use_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(ids.len(), 2);
        assert!(ids.iter().all(|id| id.starts_with("tooluse_")));
        assert_ne!(ids[0], ids[1]);
    }

    #[tokio::test]
    async fn passes_all_parameters_to_tool_execution() {
        let agent = agent_with_tools(vec![echo_tool("echo")]);
        let input = serde_json::json!({ "a": 1, "b": "two", "c": [3], "d": { "e": true } });
        let result = agent.tool("echo").unwrap().call(input.clone()).await.unwrap();

        assert_eq!(text_of(&result), input.to_string());
    }

    #[tokio::test]
    async fn can_call_a_tool_that_was_added_after_agent_creation() {
        let agent = agent_with_tools(Vec::new());
        assert!(agent.tool("late").is_err());

        agent
            .with_tool_registry(|registry| registry.add_or_replace(vec![echo_tool("late") as ToolRef]))
            .unwrap();

        let result = agent
            .tool("late")
            .unwrap()
            .call(serde_json::json!({ "x": 1 }))
            .await
            .unwrap();
        assert_eq!(result.status, ToolResultStatus::Success);
    }

    #[tokio::test]
    async fn fires_message_added_event_for_each_message_recorded_during_direct_tool_call() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        let seen: Arc<parking_lot::Mutex<Vec<Role>>> = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let captured = Arc::clone(&seen);
        agent
            .hooks()
            .add_sync_callback(move |event: &mut MessageAddedEvent| {
                captured.lock().push(event.message.role);
                Ok(())
            });

        agent
            .tool("calculator")
            .unwrap()
            .call(serde_json::json!({ "a": 1 }))
            .await
            .unwrap();

        assert_eq!(*seen.lock(), vec![Role::Assistant, Role::User, Role::Assistant]);
    }

    #[tokio::test]
    async fn does_not_fire_message_added_event_when_record_direct_tool_call_is_false() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        let count = Arc::new(AtomicBool::new(false));
        let captured = Arc::clone(&count);
        agent
            .hooks()
            .add_sync_callback(move |_event: &mut MessageAddedEvent| {
                captured.store(true, Ordering::SeqCst);
                Ok(())
            });

        agent
            .tool("calculator")
            .unwrap()
            .call_with(
                serde_json::json!({ "a": 1 }),
                CallOptions {
                    record_direct_tool_call: false,
                    ..CallOptions::default()
                },
            )
            .await
            .unwrap();

        assert!(!count.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn uses_the_agent_cancel_token_for_the_call() {
        let agent = agent_with_tools(vec![echo_tool("calculator")]);
        let token: CancellationToken = agent.cancel_token();
        assert!(!token.is_cancelled());
    }
}
