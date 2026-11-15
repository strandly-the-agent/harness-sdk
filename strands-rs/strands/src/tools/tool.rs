//! The tool interface (TS `tools/tool.ts`).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::Stream;
use futures::stream::BoxStream;
use serde::Serialize;

use crate::cancel::CancelSignal;
use crate::error::BoxError;
use crate::interrupt::{Interrupt, InterruptParams, InterruptSource};
use crate::types::agent::{AgentStreamEvent, InvocationState, ToolAgent};
use crate::types::json::JsonValue;
use crate::types::messages::{ToolResultBlock, ToolResultContent};
use crate::types::tools::{ToolResultStatus, ToolSpec, ToolUse};

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

    /// Whether this is an agent tool with `delegate` set (TS `tool instanceof AgentAsTool &&
    /// tool.delegate`).
    #[doc(hidden)]
    fn is_delegate(&self) -> bool {
        false
    }
}

impl fmt::Debug for dyn Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tool").field("name", &self.name()).finish()
    }
}

/// A progress event a tool emits while running (TS `ToolStreamEvent`).
///
/// Serializes as `{"type":"toolStreamEvent","data":..}`; equality compares `data`.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(tag = "type", rename = "toolStreamEvent")]
#[non_exhaustive]
pub struct ToolStreamEvent {
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<JsonValue>,
    /// A delegate agent's own event, which the parent's stream yields in place of this one.
    #[serde(skip)]
    delegated: Option<Arc<AgentStreamEvent>>,
}

impl PartialEq for ToolStreamEvent {
    fn eq(&self, other: &Self) -> bool {
        self.data == other.data
    }
}

impl ToolStreamEvent {
    /// An event carrying `data`.
    pub fn new(data: JsonValue) -> Self {
        Self {
            data: Some(data),
            delegated: None,
        }
    }

    /// An event carrying `event` as its data, which a parent surfaces as its own event when the
    /// tool delegates (TS `AgentDelegation` unwrapping).
    pub(crate) fn wrapping(event: AgentStreamEvent, delegated: bool) -> Self {
        let data = serde_json::to_value(&event).unwrap_or_default();
        Self {
            data: Some(data),
            delegated: delegated.then(|| Arc::new(event)),
        }
    }

    pub(crate) fn delegated(&self) -> Option<&AgentStreamEvent> {
        self.delegated.as_deref()
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

    /// The request this tool call belongs to; pass it as
    /// [`InvokeOptions::invocation_context`](crate::InvokeOptions::invocation_context) to run a
    /// nested call in the same request. `None` for a direct tool call.
    pub fn invocation_context(&self) -> Option<&crate::agent::InvocationContext> {
        self.agent.invocation_context()
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

/// Why a tool failed, or the interrupt it raised (anyhow-like).
///
/// Any `std::error::Error` converts with `?`, and so does the [`Interrupt`] that
/// [`ToolContext::interrupt`] returns, so a tool can't swallow an interrupt by accident. A failure
/// becomes an error result (`Error: <message>`); an interrupt becomes [`ToolOutcome::Interrupt`].
///
/// ```
/// use strands::ToolError;
///
/// fn parse(input: &str) -> Result<u32, ToolError> {
///     if input.is_empty() {
///         return Err(ToolError::msg("empty input"));
///     }
///     Ok(input.parse::<u32>()?)
/// }
/// assert_eq!(parse("").unwrap_err().to_string(), "empty input");
/// assert!(parse("x").is_err());
/// ```
pub struct ToolError {
    kind: ToolErrorKind,
}

enum ToolErrorKind {
    Failed(BoxError),
    Interrupt(Box<Interrupt>),
}

#[derive(Debug)]
struct MessageError(String);

impl fmt::Display for MessageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for MessageError {}

impl ToolError {
    /// A failure with `message`.
    pub fn msg(message: impl fmt::Display) -> Self {
        Self {
            kind: ToolErrorKind::Failed(Box::new(MessageError(message.to_string()))),
        }
    }

    /// The raised interrupt, when this is one.
    pub fn interrupt(&self) -> Option<&Interrupt> {
        match &self.kind {
            ToolErrorKind::Interrupt(interrupt) => Some(interrupt),
            ToolErrorKind::Failed(_) => None,
        }
    }

    /// The outcome a tool run ends with: an error result, or the interrupt (TS
    /// `createErrorResult`).
    pub fn into_outcome(self, tool_use_id: &str) -> ToolOutcome {
        match self.kind {
            ToolErrorKind::Interrupt(interrupt) => ToolOutcome::Interrupt(*interrupt),
            ToolErrorKind::Failed(error) => ToolOutcome::Result(error_result(tool_use_id, error)),
        }
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for ToolError {
    fn from(error: E) -> Self {
        Self {
            kind: ToolErrorKind::Failed(Box::new(error)),
        }
    }
}

impl From<Interrupt> for ToolError {
    fn from(interrupt: Interrupt) -> Self {
        Self {
            kind: ToolErrorKind::Interrupt(Box::new(interrupt)),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ToolErrorKind::Failed(error) => fmt::Display::fmt(error, f),
            ToolErrorKind::Interrupt(interrupt) => write!(f, "interrupt {}", interrupt.name()),
        }
    }
}

impl fmt::Debug for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            ToolErrorKind::Failed(error) => f.debug_tuple("ToolError").field(error).finish(),
            ToolErrorKind::Interrupt(interrupt) => f.debug_tuple("ToolError").field(interrupt).finish(),
        }
    }
}

/// An error-status result with the content `Error: <message>` (TS `createErrorResult`).
pub(crate) fn error_result(tool_use_id: &str, error: impl Into<BoxError>) -> ToolResultBlock {
    let error = error.into();
    ToolResultBlock::new(
        tool_use_id,
        ToolResultStatus::Error,
        vec![ToolResultContent::from(format!("Error: {error}"))],
    )
    .with_error(error)
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
