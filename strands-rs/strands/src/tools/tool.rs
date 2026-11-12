//! The tool trait and tool streams (TS `tools/tool.ts`).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::interrupt::Interrupt;
use crate::models::BoxStream;
use crate::tools::{ToolContext, ToolResultStatus, ToolSpec};
use crate::types::{JsonValue, TextBlock, ToolResultBlock, ToolResultContent};

/// Maximum tool name length (TS `MAX_TOOL_NAME_LENGTH`).
pub const MAX_TOOL_NAME_LENGTH: usize = 64;

/// Something the model can call (TS `Tool`).
///
/// Most tools come from [`#[tool]`](crate::tool) or [`FunctionTool`](crate::tools::FunctionTool);
/// implement this trait for full control over streaming. A failing tool returns an error
/// result, never an `Err`.
///
/// ```
/// use strands::tools::{Tool, ToolSpec, ToolStream};
/// use strands::{ToolContext, ToolOutcome};
/// use strands::tools::ToolResultStatus;
/// use strands::types::{ToolResultBlock, ToolResultContent};
///
/// struct Ping;
///
/// impl Tool for Ping {
///     fn name(&self) -> &str { "ping" }
///     fn description(&self) -> &str { "Replies with pong." }
///     fn tool_spec(&self) -> ToolSpec { ToolSpec::new("ping", "Replies with pong.") }
///     fn stream(&self, context: ToolContext) -> ToolStream {
///         ToolStream::from_future(async move {
///             let content = vec![ToolResultContent::from("pong")];
///             let id = &context.tool_use().tool_use_id;
///             ToolOutcome::from(ToolResultBlock::new(id, ToolResultStatus::Success, content))
///         })
///     }
/// }
/// ```
pub trait Tool: Send + Sync + 'static {
    /// The model-facing name.
    fn name(&self) -> &str;
    /// What the tool does, for the model.
    fn description(&self) -> &str;
    /// The spec sent to the model.
    fn tool_spec(&self) -> ToolSpec;
    /// Runs the tool. The stream yields progress events and ends with exactly one outcome.
    fn stream(&self, context: ToolContext) -> ToolStream;
}

impl fmt::Debug for dyn Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tool")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

/// A progress event a tool streams while it runs (TS `ToolStreamEvent`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "toolStreamEvent")]
#[non_exhaustive]
pub struct ToolStreamEvent {
    /// The event payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

impl ToolStreamEvent {
    /// An event carrying `data`.
    pub fn new(data: JsonValue) -> Self {
        Self { data: Some(data) }
    }
}

/// How a tool run ended.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ToolOutcome {
    /// The tool finished with a result (success or error status).
    Result(ToolResultBlock),
    /// The tool paused for a human answer; it re-runs from the start on resume.
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
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum ToolStreamItem {
    /// A progress event.
    Event(ToolStreamEvent),
    /// The final outcome; nothing follows it.
    Outcome(ToolOutcome),
}

/// The stream a [`Tool`] returns: events, then one [`ToolOutcome`].
///
/// A stream that ends without an outcome becomes an error result.
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

    /// A tool that streams no events and ends with the future's outcome.
    pub fn from_future(future: impl Future<Output = ToolOutcome> + Send + 'static) -> Self {
        Self::new(futures::stream::once(future).map(ToolStreamItem::Outcome))
    }
}

impl Stream for ToolStream {
    type Item = ToolStreamItem;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        self.inner.as_mut().poll_next(cx)
    }
}

impl fmt::Debug for ToolStream {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolStream").finish_non_exhaustive()
    }
}

/// Whether `name` is a valid tool name: 1 to 64 of `[a-zA-Z0-9_-]` (TS `isValidToolName`).
pub fn is_valid_tool_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_TOOL_NAME_LENGTH
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// The error result for a failed tool: `Error: <message>` with the error attached (TS
/// `createErrorResult`).
pub(crate) fn create_error_result(
    error: impl Into<crate::error::BoxError>,
    tool_use_id: &str,
) -> ToolResultBlock {
    let error: Arc<dyn std::error::Error + Send + Sync> = Arc::from(error.into());
    let mut result = error_result(tool_use_id, format!("Error: {error}"));
    result.error = Some(error);
    result
}

/// An error-status result with one text block.
pub(crate) fn error_result(tool_use_id: &str, text: impl Into<String>) -> ToolResultBlock {
    ToolResultBlock::new(
        tool_use_id,
        ToolResultStatus::Error,
        vec![ToolResultContent::Text(TextBlock::new(text))],
    )
}

#[cfg(test)]
mod tests;
