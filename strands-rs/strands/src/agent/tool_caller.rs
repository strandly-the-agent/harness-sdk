//! Direct tool calls: `agent.tool("name").invoke(input)` (TS `agent/tool-caller.ts`).

use std::sync::Arc;

use futures::StreamExt;
use futures::stream::BoxStream;

use crate::agent::agent::Agent;
use crate::agent::invocation_state::InvocationState;
use crate::agent::tool_agent::{ToolAgent, ToolAgentParts};
use crate::cancel::CancelSignal;
use crate::error::Result;
use crate::hooks::events::MessageAddedEvent;
use crate::tools::context::ToolContext;
use crate::tools::executors::executor::{PipelineToolCaller, error_result};
use crate::tools::tool::{ToolOutcome, ToolStreamItem};
use crate::types::json::JsonValue;
use crate::types::messages::{ContentBlock, Message, Role, TextBlock, ToolResultBlock, ToolUseBlock};
use crate::types::tools::ToolUse;

const DIRECT_CALL_INTERRUPT: &str = "Error: Interrupts are not supported in direct tool calls";

/// Options of one direct tool call (TS `DirectToolCallOptions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, bon::Builder)]
pub struct DirectToolCallOptions {
    /// Whether to record the call in the conversation history, as a tool use, its result and an
    /// assistant note. Defaults to `true`.
    #[builder(default = true)]
    pub record_direct_tool_call: bool,
}

impl Default for DirectToolCallOptions {
    fn default() -> Self {
        Self {
            record_direct_tool_call: true,
        }
    }
}

/// A registry tool called directly, outside the agent loop (TS `ToolHandle`).
///
/// Get one with [`Agent::tool`]. The call runs the tool alone: no hooks, no model. By default it is
/// recorded in the history so the model sees it on the next invocation.
///
/// ```
/// use strands::tools::FunctionTool;
/// use strands::testing::{MockModel, test_agent};
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), strands::Error> {
/// let add = FunctionTool::new("add", "Adds two numbers", |input, _context| async move {
///     Ok::<_, strands::ToolError>(serde_json::json!(input["a"].as_i64().unwrap_or(0) + input["b"].as_i64().unwrap_or(0)))
/// });
/// let mut agent = test_agent(MockModel::new()).tools([add]).build()?;
/// let result = agent.tool("add").invoke(serde_json::json!({ "a": 5, "b": 3 })).await?;
/// assert_eq!(serde_json::to_value(&result.content)?, serde_json::json!([{ "text": "8" }]));
/// assert_eq!(agent.messages().len(), 3);
/// # Ok(())
/// # }
/// ```
#[derive(Debug)]
pub struct ToolHandle<'a> {
    agent: &'a mut Agent,
    name: String,
}

impl Agent {
    /// A handle that calls the registry tool `name` directly (TS `agent.tool.<name>`). The name
    /// resolves as the model's tool calls do: exact, then `_` as `-`, then case-insensitive.
    pub fn tool(&mut self, name: impl Into<String>) -> ToolHandle<'_> {
        ToolHandle {
            agent: self,
            name: name.into(),
        }
    }
}

impl<'a> ToolHandle<'a> {
    /// Calls the tool with `input` and returns its result (TS `invoke`).
    ///
    /// # Errors
    ///
    /// [`ToolNotFoundError`](crate::error::ToolNotFoundError) when no registry tool has the name.
    /// A failing tool is an error result, not an error.
    pub async fn invoke(self, input: JsonValue) -> Result<ToolResultBlock> {
        self.invoke_with(input, DirectToolCallOptions::default()).await
    }

    /// [`invoke`](Self::invoke) with options.
    ///
    /// # Errors
    ///
    /// As [`invoke`](Self::invoke).
    pub async fn invoke_with(
        self,
        input: JsonValue,
        options: DirectToolCallOptions,
    ) -> Result<ToolResultBlock> {
        let mut stream = self.stream_with(input, options)?;
        let mut result = None;
        while let Some(item) = stream.next().await {
            if let ToolStreamItem::Outcome(ToolOutcome::Result(block)) = item {
                result = Some(block);
            }
        }
        Ok(result.unwrap_or_else(|| error_result("", DIRECT_CALL_INTERRUPT)))
    }

    /// Calls the tool and streams its progress events, ending with its result (TS `stream`).
    ///
    /// # Errors
    ///
    /// As [`invoke`](Self::invoke).
    pub fn stream(self, input: JsonValue) -> Result<BoxStream<'a, ToolStreamItem>> {
        self.stream_with(input, DirectToolCallOptions::default())
    }

    /// [`stream`](Self::stream) with options.
    ///
    /// # Errors
    ///
    /// As [`invoke`](Self::invoke).
    pub fn stream_with(
        self,
        input: JsonValue,
        options: DirectToolCallOptions,
    ) -> Result<BoxStream<'a, ToolStreamItem>> {
        let agent = self.agent;
        let tool = Arc::clone(agent.local.tool_registry().resolve(&self.name)?);
        let tool_use = ToolUse::new(tool.name(), format!("tooluse_{}", uuid::Uuid::new_v4()), input);
        let cancel_signal = agent.cancel.handle().signal();
        let state = InvocationState::new();
        let context = ToolContext::new(
            tool_use.clone(),
            direct_call_agent(agent, &state, &cancel_signal),
            state,
            cancel_signal,
        );
        Ok(Box::pin(async_stream::stream! {
            let mut events = tool.stream(context).until_outcome(&tool_use);
            let mut result = None;
            while let Some(item) = events.next().await {
                match item {
                    ToolStreamItem::Event(event) => yield ToolStreamItem::Event(event),
                    ToolStreamItem::Outcome(ToolOutcome::Result(block)) => result = Some(block),
                    ToolStreamItem::Outcome(ToolOutcome::Interrupt(_)) => {
                        result = Some(error_result(&tool_use.tool_use_id, DIRECT_CALL_INTERRUPT));
                    }
                }
            }
            let result = result.unwrap_or_else(|| error_result(&tool_use.tool_use_id, DIRECT_CALL_INTERRUPT));
            if options.record_direct_tool_call {
                record(agent, &tool_use, &result).await;
            }
            yield ToolStreamItem::Outcome(ToolOutcome::Result(result));
        }))
    }
}

fn direct_call_agent(agent: &Agent, state: &InvocationState, cancel_signal: &CancelSignal) -> ToolAgent {
    let local = &agent.local;
    ToolAgent::new(ToolAgentParts {
        id: local.id().to_owned(),
        name: local.name().to_owned(),
        app_state: local.app_state().clone(),
        model: Arc::clone(local.model()),
        aux_model: Arc::clone(local.aux_model()),
        cancel_handle: cancel_signal.handle(),
        messages: Arc::from(local.messages()),
        tool_registry: Arc::new(local.tool_registry().clone()),
        invocation_context: None,
        tool_caller: PipelineToolCaller::new(Arc::clone(local.hooks()), state.clone(), cancel_signal.clone()),
    })
}

/// Appends the call to the history (TS `_recordToolExecution`), firing `MessageAddedEvent` for each
/// message.
async fn record(agent: &mut Agent, tool_use: &ToolUse, result: &ToolResultBlock) {
    let use_block = ToolUseBlock::new(
        tool_use.name.clone(),
        tool_use.tool_use_id.clone(),
        tool_use.input.clone(),
    );
    let messages = [
        Message::new(Role::Assistant, vec![use_block.into()]),
        Message::new(Role::User, vec![ContentBlock::from(result.clone())]),
        Message::new(
            Role::Assistant,
            vec![TextBlock::new(format!("agent.tool.{} was called.", tool_use.name)).into()],
        ),
    ];
    let hooks = Arc::clone(agent.local.hooks());
    for message in messages {
        agent.local.messages_mut().push(message.clone());
        let mut event = MessageAddedEvent::new(&mut agent.local, message, InvocationState::new(), None);
        hooks.invoke::<MessageAddedEvent<'static>>(&mut event).await;
    }
}
