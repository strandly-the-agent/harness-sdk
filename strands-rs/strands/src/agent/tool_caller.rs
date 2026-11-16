//! Direct tool calls (TS `agent/tool-caller.ts`): `agent.tool("name").invoke(input)` runs a
//! registered tool without the model and records the call in the history.

use std::fmt;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::stream::BoxStream;
use futures::{Stream, StreamExt};

use crate::agent::Agent;
use crate::error::{Error, Result};
use crate::hooks::MessageAddedEvent;
use crate::tools::executors::executor::{error_text_result, panic_message};
use crate::tools::{ToolContext, ToolOutcome, ToolStreamItem};
use crate::types::agent::InvocationState;
use crate::types::json::JsonValue;
use crate::types::messages::{Message, Role, TextBlock, ToolResultBlock, ToolUseBlock};
use crate::types::tools::ToolUse;

/// TS throws this from `context.interrupt` in a direct call.
const INTERRUPT_UNSUPPORTED: &str = "Interrupts are not supported in direct tool calls";

/// Options for one direct tool call (TS `DirectToolCallOptions`).
///
/// ```
/// use strands::agent::DirectToolCallOptions;
///
/// assert!(DirectToolCallOptions::default().record_direct_tool_call);
/// let options = DirectToolCallOptions::builder().record_direct_tool_call(false).build();
/// assert!(!options.record_direct_tool_call);
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
pub struct DirectToolCallOptions {
    /// Whether to record the call in the agent's history (default `true`). `false` runs the tool
    /// without touching the conversation.
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

/// A registered tool, ready to be called directly (TS `ToolHandle`); from [`Agent::tool`].
///
/// The name resolves like a model-supplied one (exact, then `_` for `-`, then case-insensitive).
#[must_use = "a tool handle does nothing until invoked or streamed"]
pub struct ToolHandle<'a> {
    agent: &'a mut Agent,
    name: String,
}

impl fmt::Debug for ToolHandle<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolHandle")
            .field("name", &self.name)
            .finish_non_exhaustive()
    }
}

impl<'a> ToolHandle<'a> {
    /// Runs the tool on `input` and returns its result, recording the call in the history.
    ///
    /// # Errors
    ///
    /// No registered tool matches the name ([`Error::ToolNotFound`]), the tool panicked, or a
    /// `MessageAddedEvent` hook raised duplicate interrupts.
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
        let mut stream = self.stream_with(input, options);
        while let Some(item) = stream.next().await {
            if let ToolStreamItem::Outcome(ToolOutcome::Result(result)) = item? {
                return Ok(result);
            }
        }
        Err(Error::other("the tool call stream ended without a result"))
    }

    /// Runs the tool on `input`, yielding its progress events; the last item is
    /// `ToolStreamItem::Outcome(ToolOutcome::Result(..))`.
    pub fn stream(self, input: JsonValue) -> ToolCallStream<'a> {
        self.stream_with(input, DirectToolCallOptions::default())
    }

    /// [`stream`](Self::stream) with options.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "takes its options by value like every other options parameter"
    )]
    pub fn stream_with(self, input: JsonValue, options: DirectToolCallOptions) -> ToolCallStream<'a> {
        let Self { agent, name } = self;
        ToolCallStream {
            inner: Box::pin(async_stream::stream! {
                // The hooks a dropped stream deferred come first, so they see history in order.
                if let Err(error) = agent.finish_abandoned_run().await {
                    yield Err(error);
                    return;
                }
                let tool = match agent.local.tool_registry().resolve(&name) {
                    Ok(tool) => Arc::clone(tool),
                    Err(error) => {
                        yield Err(error);
                        return;
                    }
                };
                let tool_use = ToolUse::new(
                    tool.name(),
                    format!("tooluse_{}", uuid::Uuid::now_v7()),
                    input,
                );
                let context = ToolContext::new(
                    tool_use.clone(),
                    agent.local.direct_tool_agent(),
                    InvocationState::new(),
                );
                let events = match catch_unwind(AssertUnwindSafe(|| tool.stream(context))) {
                    Ok(events) => events,
                    Err(panic) => {
                        yield Err(Error::other(panic_message(panic.as_ref())));
                        return;
                    }
                };
                let mut events = AssertUnwindSafe(events).catch_unwind();
                let mut result = None;
                loop {
                    match events.next().await {
                        Some(Ok(ToolStreamItem::Outcome(outcome))) => {
                            result = Some(outcome_result(outcome, tool_use.tool_use_id()));
                            break;
                        }
                        Some(Ok(item)) => yield Ok(item),
                        Some(Err(panic)) => {
                            yield Err(Error::other(panic_message(panic.as_ref())));
                            return;
                        }
                        None => break,
                    }
                }
                let result = result.unwrap_or_else(|| {
                    let text = format!("Tool '{}' did not return a result", tool.name());
                    error_text_result(tool_use.tool_use_id(), text)
                });
                if options.record_direct_tool_call
                    && let Err(error) = agent.record_tool_call(&tool_use, &result).await
                {
                    yield Err(error);
                    return;
                }
                yield Ok(ToolStreamItem::Outcome(ToolOutcome::Result(result)));
            }),
        }
    }
}

/// The items of one direct tool call: the tool's progress events, then its result.
///
/// Borrows the agent mutably, like [`AgentStream`](crate::AgentStream). An error item ends the
/// stream.
#[must_use = "streams do nothing unless polled"]
pub struct ToolCallStream<'a> {
    inner: BoxStream<'a, Result<ToolStreamItem>>,
}

impl Stream for ToolCallStream<'_> {
    type Item = Result<ToolStreamItem>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.poll_next_unpin(cx)
    }
}

impl fmt::Debug for ToolCallStream<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolCallStream").finish_non_exhaustive()
    }
}

/// The result of a direct call; an interrupt fails the call as TS's throwing `interrupt` does
/// inside a function tool.
fn outcome_result(outcome: ToolOutcome, tool_use_id: &str) -> ToolResultBlock {
    match outcome {
        ToolOutcome::Result(result) => result,
        ToolOutcome::Interrupt(_) => crate::tools::tool::error_result(tool_use_id, INTERRUPT_UNSUPPORTED),
    }
}

impl Agent {
    /// A handle to call the registered tool `name` directly, without the model (TS
    /// `agent.tool.<name>`).
    ///
    /// ```
    /// use serde_json::json;
    /// use strands::tools::FunctionTool;
    /// use strands::testing::MockMessageModel;
    /// use strands::Agent;
    ///
    /// # tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(async {
    /// let add = FunctionTool::new("add", "Adds two numbers.", |input, _ctx| async move {
    ///     Ok::<_, strands::ToolError>(input["a"].as_i64().unwrap_or(0) + input["b"].as_i64().unwrap_or(0))
    /// });
    /// let mut agent = Agent::builder().model(MockMessageModel::new()).tools([add]).build()?;
    ///
    /// let result = agent.tool("add").invoke(json!({"a": 5, "b": 3})).await?;
    /// assert_eq!(serde_json::to_value(&result)?["content"], json!([{"text": "8"}]));
    /// assert_eq!(agent.messages().len(), 3);
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// # }).unwrap();
    /// ```
    pub fn tool(&mut self, name: impl Into<String>) -> ToolHandle<'_> {
        ToolHandle {
            agent: self,
            name: name.into(),
        }
    }

    /// Appends the tool use, its result and an acknowledgement, firing `MessageAddedEvent` for
    /// each (TS `_recordToolExecution`).
    async fn record_tool_call(&mut self, tool_use: &ToolUse, result: &ToolResultBlock) -> Result<()> {
        let tool_use_block =
            ToolUseBlock::new(tool_use.name(), tool_use.tool_use_id(), tool_use.input().clone());
        let messages = [
            Message::new(Role::Assistant, vec![tool_use_block.into()]),
            Message::new(Role::User, vec![result.clone().into()]),
            Message::new(
                Role::Assistant,
                vec![TextBlock::new(format!("agent.tool.{} was called.", tool_use.name())).into()],
            ),
        ];
        let hooks = self.local.hooks().clone();
        // TS fires these events without an invocation context.
        let context = self.local.take_invocation_context();
        let mut outcome = Ok(());
        for message in messages {
            self.local.messages_mut().push(message.clone());
            let mut event = MessageAddedEvent::new(&mut self.local, message, InvocationState::new());
            outcome = hooks.invoke::<MessageAddedEvent<'static>>(&mut event).await;
            if outcome.is_err() {
                break;
            }
        }
        self.local.set_invocation_context(context);
        outcome
    }
}

#[cfg(test)]
mod tests;
