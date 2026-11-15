//! The `Tool` trait and its stream types (TS `tools/tool.ts`).

use std::error::Error as StdError;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::task::{Context, Poll};

use futures::{Stream, StreamExt};
use serde::{Deserialize, Serialize};

use crate::interrupt::Interrupt;
use crate::models::model::BoxStream;
use crate::tools::context::ToolContext;
use crate::types::json::JsonValue;
use crate::types::messages::{TextBlock, ToolResultBlock};
use crate::types::tools::{ToolResultStatus, ToolSpec, ToolUse};

/// The longest tool name a provider accepts.
pub(crate) const MAX_TOOL_NAME_LENGTH: usize = 64;

/// A tool the model can call (TS `Tool`).
///
/// Most tools come from [`#[tool]`](macro@crate::tool), [`#[tools]`](macro@crate::tools) or
/// [`FunctionTool`](crate::tools::FunctionTool). Implement the trait directly to stream progress:
///
/// ```
/// use futures::stream;
/// use strands::tools::{Tool, ToolStream, ToolStreamEvent, ToolStreamItem};
/// use strands::types::messages::ToolResultBlock;
/// use strands::types::tools::{ToolResultStatus, ToolSpec};
/// use strands::{ToolContext, ToolOutcome};
///
/// struct Countdown;
///
/// impl Tool for Countdown {
///     fn name(&self) -> &str {
///         "countdown"
///     }
///
///     fn description(&self) -> &str {
///         "Counts down from three."
///     }
///
///     fn tool_spec(&self) -> ToolSpec {
///         ToolSpec::new(self.name(), self.description())
///     }
///
///     fn stream(&self, context: ToolContext) -> ToolStream {
///         let id = context.tool_use().tool_use_id.clone();
///         let events = (1..=3).rev().map(|n| ToolStreamItem::Event(ToolStreamEvent::new().with_data(n.into())));
///         let result = ToolResultBlock::new(id, ToolResultStatus::Success, vec!["liftoff".into()]);
///         ToolStream::new(stream::iter(events.chain([ToolStreamItem::Outcome(ToolOutcome::Result(result))])))
///     }
/// }
/// ```
pub trait Tool: Send + Sync + 'static {
    /// The unique name the model calls the tool by. Matches [`ToolSpec::name`].
    fn name(&self) -> &str;

    /// What the tool does, so the model knows when to use it. Matches [`ToolSpec::description`].
    fn description(&self) -> &str;

    /// The specification sent to the model: name, description and input schema.
    fn tool_spec(&self) -> ToolSpec;

    /// Runs the tool. The stream yields zero or more progress events and ends with exactly one
    /// [`ToolStreamItem::Outcome`]. A failure is an error-status result, never a panic.
    fn stream(&self, context: ToolContext) -> ToolStream;
}

impl fmt::Debug for dyn Tool {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Tool")
            .field("name", &self.name())
            .finish_non_exhaustive()
    }
}

/// Progress a tool reports while it runs (TS `ToolStreamEvent`), written as
/// `{"type":"toolStreamEvent","data":..}`.
///
/// ```
/// use serde_json::json;
/// use strands::tools::ToolStreamEvent;
///
/// let event = ToolStreamEvent::new().with_data(json!({ "progress": 50 }));
/// assert_eq!(
///     serde_json::to_value(&event).unwrap(),
///     json!({ "type": "toolStreamEvent", "data": { "progress": 50 } })
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename = "toolStreamEvent")]
pub struct ToolStreamEvent {
    /// The progress data, whatever the tool chooses to report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<JsonValue>,
}

impl ToolStreamEvent {
    /// An event without data.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the progress data.
    #[must_use]
    pub fn with_data(mut self, data: JsonValue) -> Self {
        self.data = Some(data);
        self
    }
}

/// How a tool call ended: a result, or an interrupt asking the caller for input.
///
/// A failed call is a [`Result`](Self::Result) with error status. Build one from a
/// [`ToolError`] with [`ToolOutcome::from_error`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ToolOutcome {
    /// The call finished with this result.
    Result(ToolResultBlock),
    /// The call paused on this interrupt; it re-runs from the start on resume.
    Interrupt(Interrupt),
}

impl ToolOutcome {
    /// The outcome of a call that failed with `error`: the interrupt it carries, or an error
    /// result whose text is `Error: <message>` (TS `createErrorResult`).
    ///
    /// ```
    /// use strands::{ToolError, ToolOutcome};
    ///
    /// let ToolOutcome::Result(result) = ToolOutcome::from_error("t-1", ToolError::msg("disk full")) else {
    ///     unreachable!()
    /// };
    /// assert_eq!(serde_json::to_value(&result.content).unwrap(), serde_json::json!([{ "text": "Error: disk full" }]));
    /// ```
    #[must_use]
    pub fn from_error(tool_use_id: impl Into<String>, error: ToolError) -> Self {
        match error.repr {
            Repr::Interrupt(interrupt) => Self::Interrupt(*interrupt),
            Repr::Error(error) => Self::Result(error_result(tool_use_id, error)),
        }
    }
}

impl From<ToolResultBlock> for ToolOutcome {
    fn from(result: ToolResultBlock) -> Self {
        Self::Result(result)
    }
}

/// An item of a [`ToolStream`].
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ToolStreamItem {
    /// Progress reported while the tool runs.
    Event(ToolStreamEvent),
    /// How the call ended; always the last item.
    Outcome(ToolOutcome),
}

/// The stream a [`Tool`] returns: progress events, then one [`ToolOutcome`].
///
/// A stream that ends without an outcome counts as an error result. It is `'static`, so tools
/// clone what they need out of `&self`.
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

    /// A stream with no progress events whose outcome is the output of `future`.
    ///
    /// ```
    /// use strands::tools::ToolStream;
    /// use strands::types::messages::ToolResultBlock;
    /// use strands::types::tools::ToolResultStatus;
    /// use strands::ToolOutcome;
    ///
    /// let stream = ToolStream::from_future(async {
    ///     ToolOutcome::Result(ToolResultBlock::new("t-1", ToolResultStatus::Success, vec!["done".into()]))
    /// });
    /// # drop(stream);
    /// ```
    pub fn from_future(future: impl Future<Output = ToolOutcome> + Send + 'static) -> Self {
        Self::new(futures::stream::once(future).map(ToolStreamItem::Outcome))
    }

    /// Ends the stream at its first outcome, and adds the TS "did not return a result" error
    /// result when it ends without one.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by the tool executors"))]
    pub(crate) fn until_outcome(self, tool_use: &ToolUse) -> Self {
        let missing = missing_outcome(tool_use);
        Self::new(futures::stream::unfold(
            (self.inner, Some(missing)),
            |(mut inner, missing)| async move {
                let missing = missing?;
                match inner.next().await {
                    Some(ToolStreamItem::Outcome(outcome)) => {
                        Some((ToolStreamItem::Outcome(outcome), (inner, None)))
                    }
                    Some(item) => Some((item, (inner, Some(missing)))),
                    None => Some((ToolStreamItem::Outcome(missing), (inner, None))),
                }
            },
        ))
    }

    /// Drains the stream, returning its progress events and its outcome.
    #[cfg_attr(not(test), expect(dead_code, reason = "used by direct tool calls"))]
    pub(crate) async fn collect_outcome(self, tool_use: &ToolUse) -> (Vec<ToolStreamEvent>, ToolOutcome) {
        let mut stream = self.until_outcome(tool_use);
        let mut events = Vec::new();
        while let Some(item) = stream.next().await {
            match item {
                ToolStreamItem::Event(event) => events.push(event),
                ToolStreamItem::Outcome(outcome) => return (events, outcome),
            }
        }
        (events, missing_outcome(tool_use))
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

fn missing_outcome(tool_use: &ToolUse) -> ToolOutcome {
    ToolOutcome::Result(ToolResultBlock::new(
        tool_use.tool_use_id.clone(),
        ToolResultStatus::Error,
        vec![format!("Tool '{}' did not return a result", tool_use.name).into()],
    ))
}

/// Why a tool call failed: any error, or an [`Interrupt`] raised through
/// [`ToolContext::interrupt`].
///
/// Like `anyhow::Error`, it converts from every [`std::error::Error`] with `?`, and from an
/// [`Interrupt`], so `ctx.interrupt(..)?` pauses the call. [`ToolError::msg`] makes one from a
/// message. It deliberately doesn't implement [`std::error::Error`] itself.
///
/// ```
/// use strands::ToolError;
///
/// fn parse(text: &str) -> Result<u32, ToolError> {
///     if text.is_empty() {
///         return Err(ToolError::msg("empty input"));
///     }
///     Ok(text.parse::<u32>()?)
/// }
///
/// assert_eq!(parse("").unwrap_err().to_string(), "empty input");
/// assert_eq!(parse("x").unwrap_err().to_string(), "invalid digit found in string");
/// ```
pub struct ToolError {
    repr: Repr,
}

enum Repr {
    Error(Box<dyn StdError + Send + Sync + 'static>),
    Interrupt(Box<Interrupt>),
}

impl ToolError {
    /// An error with this message.
    pub fn msg(message: impl fmt::Display + fmt::Debug + Send + Sync + 'static) -> Self {
        Self {
            repr: Repr::Error(Box::new(MessageError(message))),
        }
    }

    /// The interrupt this error carries, when it came from [`ToolContext::interrupt`].
    #[must_use]
    pub fn interrupt(&self) -> Option<&Interrupt> {
        match &self.repr {
            Repr::Interrupt(interrupt) => Some(interrupt),
            Repr::Error(_) => None,
        }
    }

    /// The underlying error, unless this is an interrupt.
    #[must_use]
    pub fn error(&self) -> Option<&(dyn StdError + Send + Sync + 'static)> {
        match &self.repr {
            Repr::Error(error) => Some(&**error),
            Repr::Interrupt(_) => None,
        }
    }
}

impl<E: StdError + Send + Sync + 'static> From<E> for ToolError {
    fn from(error: E) -> Self {
        Self {
            repr: Repr::Error(Box::new(error)),
        }
    }
}

impl From<Interrupt> for ToolError {
    fn from(interrupt: Interrupt) -> Self {
        Self {
            repr: Repr::Interrupt(Box::new(interrupt)),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            Repr::Error(error) => fmt::Display::fmt(error, f),
            Repr::Interrupt(interrupt) => write!(f, "Interrupt raised: {}", interrupt.name),
        }
    }
}

impl fmt::Debug for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.repr {
            Repr::Error(error) => fmt::Debug::fmt(error, f),
            Repr::Interrupt(interrupt) => f.debug_tuple("Interrupt").field(interrupt).finish(),
        }
    }
}

struct MessageError<M>(M);

impl<M: fmt::Display> fmt::Display for MessageError<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<M: fmt::Debug> fmt::Debug for MessageError<M> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl<M: fmt::Display + fmt::Debug> StdError for MessageError<M> {}

/// An error-status result with the text `Error: <message>` that keeps `error` (TS
/// `createErrorResult`).
pub(crate) fn error_result(
    tool_use_id: impl Into<String>,
    error: Box<dyn StdError + Send + Sync + 'static>,
) -> ToolResultBlock {
    let text = TextBlock::new(format!("Error: {error}"));
    ToolResultBlock::new(tool_use_id, ToolResultStatus::Error, vec![text.into()]).with_error(error)
}

/// Whether `name` is a tool name providers accept: 1 to 64 ASCII letters, digits, `_` or `-` (TS
/// `isValidToolName`).
///
/// ```
/// use strands::tools::is_valid_tool_name;
///
/// assert!(is_valid_tool_name("get_weather"));
/// assert!(!is_valid_tool_name("has space"));
/// ```
#[must_use]
pub fn is_valid_tool_name(name: &str) -> bool {
    (1..=MAX_TOOL_NAME_LENGTH).contains(&name.len()) && name.bytes().all(is_tool_name_byte)
}

pub(crate) fn is_tool_name_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-'
}

#[cfg(test)]
mod tests;
