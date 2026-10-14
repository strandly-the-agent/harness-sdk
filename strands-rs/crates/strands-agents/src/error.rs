//! Error types. One variant per TS error class (`strands-ts/src/errors.ts`).

use crate::types::Message;

/// Boxed source error.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// All errors produced by the SDK.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// Generic model-provider failure (`ModelError`).
    #[error("{message}")]
    Model {
        /// Provider message.
        message: String,
        /// Underlying cause, if any.
        #[source]
        source: Option<BoxError>,
    },
    /// The request exceeded the model's context window (`ContextWindowOverflowError`).
    #[error("{0}")]
    ContextWindowOverflow(String),
    /// The model hit its output-token cap (`MaxTokensError`).
    #[error("{message}")]
    MaxTokens {
        /// Provider message.
        message: String,
        /// Assistant message truncated at the token cap.
        partial_message: Box<Message>,
    },
    /// The provider throttled the request (`ModelThrottledError`); retryable by default.
    #[error("{message}")]
    ModelThrottled {
        /// Provider message.
        message: String,
        /// Underlying cause, if any.
        #[source]
        source: Option<BoxError>,
    },
    /// Provider-side token counting failed (`ProviderTokenCountError`).
    #[error("{message}")]
    ProviderTokenCount {
        /// Provider message.
        message: String,
        /// Underlying cause, if any.
        #[source]
        source: Option<BoxError>,
    },
    /// A value could not be serialized to JSON (`JsonValidationError`).
    #[error("{0}")]
    JsonValidation(String),
    /// The agent was invoked while another invocation was running (`ConcurrentInvocationError`).
    #[error("{0}")]
    ConcurrentInvocation(String),
    /// Session persistence failure (`SessionError`).
    #[error("{message}")]
    Session {
        /// Message.
        message: String,
        /// Underlying cause, if any.
        #[source]
        source: Option<BoxError>,
    },
    /// Tool registration/spec validation failure (`ToolValidationError`).
    #[error("{0}")]
    ToolValidation(String),
    /// Structured output could not be produced (`StructuredOutputError`).
    #[error("{0}")]
    StructuredOutput(String),
    /// No tool with the given name is registered (`ToolNotFoundError`).
    #[error("Tool '{tool_name}' not found")]
    ToolNotFound {
        /// The requested tool name.
        tool_name: String,
    },
    /// The invocation was cancelled (`CancelledError`). Never retried.
    #[error("Agent invocation cancelled")]
    Cancelled,
    /// A default slot was used before being configured (`DefaultNotConfiguredError`).
    #[error("{0}")]
    DefaultNotConfigured(String),
    /// Storage backend failure (`StorageError`).
    #[error("{message}")]
    Storage {
        /// Message.
        message: String,
        /// Underlying cause, if any.
        #[source]
        source: Option<BoxError>,
    },
    /// Plugin registration failure.
    #[error("{0}")]
    Plugin(String),
    /// Any other error (user tool/hook failures, I/O, serde).
    #[error(transparent)]
    Other(BoxError),
}

impl Error {
    /// Generic model error with a message and no cause.
    pub fn model(message: impl Into<String>) -> Self {
        Error::Model {
            message: message.into(),
            source: None,
        }
    }

    /// Generic model error wrapping a provider error as its source.
    pub fn model_with_source(message: impl Into<String>, source: impl Into<BoxError>) -> Self {
        Error::Model {
            message: message.into(),
            source: Some(source.into()),
        }
    }

    /// `ModelThrottled` with a message and no cause.
    pub fn throttled(message: impl Into<String>) -> Self {
        Error::ModelThrottled {
            message: message.into(),
            source: None,
        }
    }

    /// Wrap any error as `Error::Other`.
    pub fn other(err: impl Into<BoxError>) -> Self {
        Error::Other(err.into())
    }

    /// `true` for every model-provider error class (`ModelError` and subclasses in TS).
    pub fn is_model_error(&self) -> bool {
        matches!(
            self,
            Error::Model { .. }
                | Error::ContextWindowOverflow(_)
                | Error::MaxTokens { .. }
                | Error::ModelThrottled { .. }
                | Error::ProviderTokenCount { .. }
        )
    }

    /// The TS class name for this error (`error.name`), used in wire shapes and logs.
    pub fn name(&self) -> &'static str {
        match self {
            Error::Model { .. } => "ModelError",
            Error::ContextWindowOverflow(_) => "ContextWindowOverflowError",
            Error::MaxTokens { .. } => "MaxTokensError",
            Error::ModelThrottled { .. } => "ModelThrottledError",
            Error::ProviderTokenCount { .. } => "ProviderTokenCountError",
            Error::JsonValidation(_) => "JsonValidationError",
            Error::ConcurrentInvocation(_) => "ConcurrentInvocationError",
            Error::Session { .. } => "SessionError",
            Error::ToolValidation(_) => "ToolValidationError",
            Error::StructuredOutput(_) => "StructuredOutputError",
            Error::ToolNotFound { .. } => "ToolNotFoundError",
            Error::Cancelled => "CancelledError",
            Error::DefaultNotConfigured(_) => "DefaultNotConfiguredError",
            Error::Storage { .. } => "StorageError",
            Error::Plugin(_) => "Error",
            Error::Other(_) => "Error",
        }
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Other(Box::new(err))
    }
}

impl From<std::io::Error> for Error {
    fn from(err: std::io::Error) -> Self {
        Error::Other(Box::new(err))
    }
}

/// Convenience alias used throughout the crate.
pub type Result<T, E = Error> = std::result::Result<T, E>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tool_not_found_message_and_name() {
        let err = Error::ToolNotFound {
            tool_name: "calc".into(),
        };
        assert_eq!(err.to_string(), "Tool 'calc' not found");
        assert_eq!(err.name(), "ToolNotFoundError");
    }

    #[test]
    fn cancelled_has_fixed_message() {
        assert_eq!(Error::Cancelled.to_string(), "Agent invocation cancelled");
    }

    #[test]
    fn model_error_family() {
        assert!(Error::throttled("slow down").is_model_error());
        assert!(Error::ContextWindowOverflow("too big".into()).is_model_error());
        assert!(!Error::ToolValidation("bad".into()).is_model_error());
    }
}
