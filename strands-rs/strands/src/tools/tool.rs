//! The tool interface (TS `tools/tool.ts`).
#![allow(
    dead_code,
    reason = "crate-internal constructors are used by the executors, which land later in Phase 3"
)]

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::Stream;
use futures::stream::BoxStream;
use serde::Serialize;

use crate::cancel::CancelSignal;
use crate::interrupt::{Interrupt, InterruptParams, InterruptSource};
use crate::types::agent::{InvocationState, ToolAgent};
use crate::types::json::JsonValue;
use crate::types::messages::ToolResultBlock;
use crate::types::tools::{ToolSpec, ToolUse};

/// A tool the model can call (TS `Tool`).
///
/// Tools are shared as `Arc<dyn Tool>`. [`Tool::stream`] returns a `'static` stream that may emit
/// progress events and ends with exactly one [`ToolOutcome`]. A failing tool ends with an
/// error-status result, never an `Err`.
pub trait Tool: Send + Sync + 'static {
    /// The model-facing tool name.
    fn name(&self) -> &str;

    /// The model-facing description.
    fn description(&self) -> &str;

    /// The specification sent to the model.
    fn tool_spec(&self) -> ToolSpec;

    /// Runs the tool for one tool use.
    fn stream(&self, context: ToolContext) -> ToolStream;
}

impl fmt::Debug for dyn Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tool").field("name", &self.name()).finish()
    }
}

/// A progress event a tool emits while running (TS `ToolStreamEvent`).
///
/// Serializes as `{"type":"toolStreamEvent","data":..}`.
#[derive(Debug, Clone, PartialEq, Default, Serialize)]
#[serde(tag = "type", rename = "toolStreamEvent")]
#[non_exhaustive]
pub struct ToolStreamEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<JsonValue>,
}

impl ToolStreamEvent {
    /// An event carrying `data`.
    pub fn new(data: JsonValue) -> Self {
        Self { data: Some(data) }
    }

    /// The event payload.
    pub fn data(&self) -> Option<&JsonValue> {
        self.data.as_ref()
    }
}

/// How a tool run ended.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ToolOutcome {
    /// The tool produced a result (success or error status).
    Result(ToolResultBlock),
    /// The tool raised an interrupt and waits for a response.
    Interrupt(Interrupt),
}

impl From<ToolResultBlock> for ToolOutcome {
    fn from(result: ToolResultBlock) -> Self {
        Self::Result(result)
    }
}

impl From<Interrupt> for ToolOutcome {
    fn from(interrupt: Interrupt) -> Self {
        Self::Interrupt(interrupt)
    }
}

/// One item of a [`ToolStream`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum ToolStreamItem {
    /// A progress event.
    Event(ToolStreamEvent),
    /// The final outcome; the stream ends after it.
    Outcome(ToolOutcome),
}

impl From<ToolStreamEvent> for ToolStreamItem {
    fn from(event: ToolStreamEvent) -> Self {
        Self::Event(event)
    }
}

impl From<ToolOutcome> for ToolStreamItem {
    fn from(outcome: ToolOutcome) -> Self {
        Self::Outcome(outcome)
    }
}

/// The stream a tool returns (TS `ToolStreamGenerator`).
///
/// A stream that ends without an [`ToolStreamItem::Outcome`] becomes an error result.
///
/// ```
/// use strands::tools::{ToolOutcome, ToolStream};
/// use strands::types::messages::ToolResultBlock;
/// use strands::types::tools::ToolResultStatus;
///
/// let stream = ToolStream::from_future(async {
///     ToolOutcome::Result(ToolResultBlock::new("tool-1", ToolResultStatus::Success, vec!["done".into()]))
/// });
/// # drop(stream);
/// ```
pub struct ToolStream {
    inner: BoxStream<'static, ToolStreamItem>,
}

impl ToolStream {
    /// Wraps a stream of items.
    pub fn new(stream: impl Stream<Item = ToolStreamItem> + Send + 'static) -> Self {
        Self {
            inner: Box::pin(stream),
        }
    }

    /// A stream with no progress events whose outcome is `future`'s output.
    pub fn from_future(future: impl Future<Output = ToolOutcome> + Send + 'static) -> Self {
        Self::new(futures::stream::once(async move {
            ToolStreamItem::Outcome(future.await)
        }))
    }
}

impl fmt::Debug for ToolStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolStream").finish_non_exhaustive()
    }
}

impl Stream for ToolStream {
    type Item = ToolStreamItem;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

/// What a running tool can reach (TS `ToolContext`).
///
/// Cheap to clone and `'static`, so tools can move it into spawned work.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ToolContext {
    tool_use: ToolUse,
    agent: ToolAgent,
    invocation_state: InvocationState,
}

impl ToolContext {
    pub(crate) fn new(tool_use: ToolUse, agent: ToolAgent, invocation_state: InvocationState) -> Self {
        Self {
            tool_use,
            agent,
            invocation_state,
        }
    }

    /// The tool use being executed.
    pub fn tool_use(&self) -> &ToolUse {
        &self.tool_use
    }

    /// The agent view for tools.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The caller's per-invocation values.
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The cancellation signal of the running invocation.
    pub fn cancel_signal(&self) -> &CancelSignal {
        self.agent.cancel_signal()
    }

    /// Asks the user for input (TS `context.interrupt`).
    ///
    /// On resume, returns the stored response. Otherwise returns the [`Interrupt`] as the error,
    /// which `?` converts into a `ToolError` that ends the tool with [`ToolOutcome::Interrupt`].
    /// The interrupt id is `tool:<toolUseId>:<name>`. An interrupted tool re-runs from the start on
    /// resume.
    ///
    /// # Errors
    ///
    /// Returns the raised [`Interrupt`] when no response is stored for it.
    #[expect(
        clippy::result_large_err,
        reason = "the interrupt is the designed error value; `?` converts it into `ToolError`"
    )]
    pub fn interrupt(&self, params: InterruptParams) -> Result<JsonValue, Interrupt> {
        let id = format!("tool:{}:{}", self.tool_use.tool_use_id(), params.name);
        self.agent
            .interrupt_state()
            .interrupt(id, params, InterruptSource::Tool)
    }
}

/// Whether `name` is a valid tool name: 1 to 64 characters of `[a-zA-Z0-9_-]` (TS
/// `isValidToolName`).
pub(crate) fn is_valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}

#[cfg(test)]
mod tests;
