//! The error type tool bodies return.

use std::fmt;

use crate::error::BoxError;
use crate::interrupt::Interrupt;
use crate::tools::ToolOutcome;
use crate::tools::tool::create_error_result;

enum Kind {
    Error(BoxError),
    Interrupt(Box<Interrupt>),
}

/// The error a tool body returns: any error, or an interrupt raised by
/// [`ToolContext::interrupt`](crate::ToolContext::interrupt).
///
/// Like `anyhow::Error`, every `std::error::Error` converts into it, so `?` works on any
/// fallible call; an [`Interrupt`] converts too, so `ctx.interrupt(..)?` pauses the tool. An
/// error becomes an error result the model sees as `Error: <message>`.
///
/// ```
/// use strands::ToolError;
///
/// fn parse(input: &str) -> Result<u32, ToolError> {
///     if input.is_empty() {
///         return Err(ToolError::msg("input is empty"));
///     }
///     Ok(input.parse::<u32>()?)
/// }
/// assert_eq!(parse("").unwrap_err().to_string(), "input is empty");
/// assert!(parse("x").is_err());
/// ```
pub struct ToolError {
    kind: Kind,
}

impl ToolError {
    /// An error with a plain message.
    pub fn msg(message: impl fmt::Display) -> Self {
        Self {
            kind: Kind::Error(message.to_string().into()),
        }
    }

    /// Whether this is an interrupt rather than a failure.
    pub fn is_interrupt(&self) -> bool {
        matches!(self.kind, Kind::Interrupt(_))
    }

    /// The outcome for the tool call `tool_use_id`: an interrupt, or an error result.
    pub fn into_outcome(self, tool_use_id: &str) -> ToolOutcome {
        match self.kind {
            Kind::Interrupt(interrupt) => ToolOutcome::Interrupt(*interrupt),
            Kind::Error(error) => ToolOutcome::Result(create_error_result(error, tool_use_id)),
        }
    }
}

impl<E: std::error::Error + Send + Sync + 'static> From<E> for ToolError {
    fn from(error: E) -> Self {
        Self {
            kind: Kind::Error(Box::new(error)),
        }
    }
}

impl From<Interrupt> for ToolError {
    fn from(interrupt: Interrupt) -> Self {
        Self {
            kind: Kind::Interrupt(Box::new(interrupt)),
        }
    }
}

impl fmt::Display for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            Kind::Error(error) => fmt::Display::fmt(error, f),
            Kind::Interrupt(interrupt) => write!(f, "interrupt {}", interrupt.name),
        }
    }
}

impl fmt::Debug for ToolError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.kind {
            Kind::Error(error) => f.debug_tuple("ToolError").field(error).finish(),
            Kind::Interrupt(interrupt) => f.debug_tuple("ToolError::Interrupt").field(interrupt).finish(),
        }
    }
}
