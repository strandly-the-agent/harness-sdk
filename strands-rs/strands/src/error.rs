//! Errors (TS `errors.ts` and `sandbox/errors.ts`).
//!
//! Every exported TS error class is a struct here with the TS message and `name`, and a variant of
//! [`Error`], the error type every fallible SDK call returns. Interrupts are never errors: an
//! interrupted run is a successful result that carries its interrupts.
//!
//! ```
//! use strands::error::{Error, ModelThrottledError};
//!
//! let error = Error::from(ModelThrottledError::new("Rate limit exceeded"));
//! assert_eq!(error.name(), "ModelThrottledError");
//! assert!(error.is_model_error());
//! assert_eq!(error.to_string(), "Rate limit exceeded");
//! ```

mod sandbox;

use crate::types::messages::Message;

pub use sandbox::{
    SandboxAbortError, SandboxIsADirectoryError, SandboxPathNotFoundError, SandboxPermissionError,
    SandboxTimeoutError,
};

/// The underlying error a TS `cause` holds.
type Source = Box<dyn std::error::Error + Send + Sync + 'static>;

/// A `Result` whose error defaults to [`Error`].
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Every error the SDK returns, one variant per TS error class.
///
/// Each variant displays the TS message and forwards [`source`](std::error::Error::source) to the
/// TS `cause`.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// See [`ModelError`].
    #[error(transparent)]
    Model(#[from] ModelError),
    /// See [`ContextWindowOverflowError`].
    #[error(transparent)]
    ContextWindowOverflow(#[from] ContextWindowOverflowError),
    /// See [`MaxTokensError`].
    #[error(transparent)]
    MaxTokens(#[from] MaxTokensError),
    /// See [`ModelThrottledError`].
    #[error(transparent)]
    ModelThrottled(#[from] ModelThrottledError),
    /// See [`ModelNetworkError`].
    #[error(transparent)]
    ModelNetwork(#[from] ModelNetworkError),
    /// See [`JsonValidationError`].
    #[error(transparent)]
    JsonValidation(#[from] JsonValidationError),
    /// See [`ToolValidationError`].
    #[error(transparent)]
    ToolValidation(#[from] ToolValidationError),
    /// See [`ToolNotFoundError`].
    #[error(transparent)]
    ToolNotFound(#[from] ToolNotFoundError),
    /// See [`StructuredOutputError`].
    #[error(transparent)]
    StructuredOutput(#[from] StructuredOutputError),
    /// See [`DefaultNotConfiguredError`].
    #[error(transparent)]
    DefaultNotConfigured(#[from] DefaultNotConfiguredError),
    /// See [`StorageError`].
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// See [`SandboxTimeoutError`].
    #[error(transparent)]
    SandboxTimeout(#[from] SandboxTimeoutError),
    /// See [`SandboxAbortError`].
    #[error(transparent)]
    SandboxAbort(#[from] SandboxAbortError),
    /// See [`SandboxPathNotFoundError`].
    #[error(transparent)]
    SandboxPathNotFound(#[from] SandboxPathNotFoundError),
    /// See [`SandboxIsADirectoryError`].
    #[error(transparent)]
    SandboxIsADirectory(#[from] SandboxIsADirectoryError),
    /// See [`SandboxPermissionError`].
    #[error(transparent)]
    SandboxPermission(#[from] SandboxPermissionError),
    /// See [`DataError`].
    #[error(transparent)]
    Data(#[from] DataError),
    /// See [`OtherError`].
    #[error(transparent)]
    Other(#[from] OtherError),
}

const _: () = {
    const fn assert_send_sync<T: Send + Sync + 'static>() {}
    assert_send_sync::<Error>();
};

impl Error {
    /// The TS `error.name` of the error, `"Error"` for [`Error::Data`] and [`Error::Other`].
    #[must_use]
    pub fn name(&self) -> &'static str {
        match self {
            Self::Model(_) => ModelError::NAME,
            Self::ContextWindowOverflow(_) => ContextWindowOverflowError::NAME,
            Self::MaxTokens(_) => MaxTokensError::NAME,
            Self::ModelThrottled(_) => ModelThrottledError::NAME,
            Self::ModelNetwork(_) => ModelNetworkError::NAME,
            Self::JsonValidation(_) => JsonValidationError::NAME,
            Self::ToolValidation(_) => ToolValidationError::NAME,
            Self::ToolNotFound(_) => ToolNotFoundError::NAME,
            Self::StructuredOutput(_) => StructuredOutputError::NAME,
            Self::DefaultNotConfigured(_) => DefaultNotConfiguredError::NAME,
            Self::Storage(_) => StorageError::NAME,
            Self::SandboxTimeout(_) => SandboxTimeoutError::NAME,
            Self::SandboxAbort(_) => SandboxAbortError::NAME,
            Self::SandboxPathNotFound(_) => SandboxPathNotFoundError::NAME,
            Self::SandboxIsADirectory(_) => SandboxIsADirectoryError::NAME,
            Self::SandboxPermission(_) => SandboxPermissionError::NAME,
            Self::Data(_) => DataError::NAME,
            Self::Other(_) => OtherError::NAME,
        }
    }

    /// Whether the error is a [`ModelError`] or one of its TS subclasses
    /// ([`ContextWindowOverflowError`], [`MaxTokensError`], [`ModelThrottledError`],
    /// [`ModelNetworkError`]).
    #[must_use]
    pub fn is_model_error(&self) -> bool {
        matches!(
            self,
            Self::Model(_)
                | Self::ContextWindowOverflow(_)
                | Self::MaxTokens(_)
                | Self::ModelThrottled(_)
                | Self::ModelNetwork(_)
        )
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Other(OtherError::Io(error))
    }
}

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Other(OtherError::Json(error))
    }
}

/// Defines a TS error class that carries only a message, optionally with a `cause`.
macro_rules! message_error {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[non_exhaustive]
        #[derive(Debug, thiserror::Error)]
        #[error("{message}")]
        pub struct $name {
            message: String,
        }

        impl $name {
            #[doc = concat!("The TS `error.name`, `\"", stringify!($name), "\"`.")]
            pub const NAME: &'static str = stringify!($name);

            /// Creates the error.
            #[must_use]
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into() }
            }

            /// The error message.
            #[must_use]
            pub fn message(&self) -> &str {
                &self.message
            }
        }
    };
    ($(#[$meta:meta])* $name:ident, cause) => {
        $(#[$meta])*
        #[non_exhaustive]
        #[derive(Debug, thiserror::Error)]
        #[error("{message}")]
        pub struct $name {
            message: String,
            source: Option<Source>,
        }

        impl $name {
            #[doc = concat!("The TS `error.name`, `\"", stringify!($name), "\"`.")]
            pub const NAME: &'static str = stringify!($name);

            /// Creates the error.
            #[must_use]
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into(), source: None }
            }

            /// Sets the underlying error (TS `cause`), returned by
            /// [`source`](std::error::Error::source).
            #[must_use]
            pub fn with_source(mut self, source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
                self.source = Some(source.into());
                self
            }

            /// The error message.
            #[must_use]
            pub fn message(&self) -> &str {
                &self.message
            }
        }
    };
}

message_error!(
    /// A model provider call failed (TS `ModelError`), the base of the model error family that
    /// [`Error::is_model_error`] recognises.
    ModelError,
    cause
);

message_error!(
    /// The input exceeds the model's context window (TS `ContextWindowOverflowError`, a
    /// `ModelError`).
    ContextWindowOverflowError
);

message_error!(
    /// The model provider rate-limited the request (TS `ModelThrottledError`, a `ModelError`). The
    /// default retry strategy retries it.
    ModelThrottledError,
    cause
);

message_error!(
    /// The connection to the model provider failed before or while streaming the response (TS
    /// `ModelNetworkError`, a `ModelError`). The default retry strategy retries it.
    ModelNetworkError,
    cause
);

message_error!(
    /// A value can't be serialized as JSON (TS `JsonValidationError`).
    JsonValidationError
);

message_error!(
    /// A tool's definition is invalid, for example its name (TS `ToolValidationError`).
    ToolValidationError
);

message_error!(
    /// Structured output failed: the model didn't use the structured output tool, or its input
    /// failed validation (TS `StructuredOutputError`).
    StructuredOutputError
);

message_error!(
    /// A required default (such as the default model) is not configured (TS
    /// `DefaultNotConfiguredError`).
    DefaultNotConfiguredError
);

message_error!(
    /// A storage operation failed, whatever the backend (TS `StorageError`).
    StorageError,
    cause
);

/// The model stopped at its output token limit (TS `MaxTokensError`, a `ModelError`). Carries
/// the partial message generated before the limit.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct MaxTokensError {
    message: String,
    partial_message: Box<Message>,
}

impl MaxTokensError {
    /// The TS `error.name`, `"MaxTokensError"`.
    pub const NAME: &'static str = "MaxTokensError";

    /// Creates the error.
    #[must_use]
    pub fn new(message: impl Into<String>, partial_message: Message) -> Self {
        Self {
            message: message.into(),
            partial_message: Box::new(partial_message),
        }
    }

    /// The error message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The partial assistant message generated before the limit (TS `partialMessage`).
    #[must_use]
    pub fn partial_message(&self) -> &Message {
        &self.partial_message
    }
}

/// No tool with the requested name is registered (TS `ToolNotFoundError`).
///
/// ```
/// use strands::error::ToolNotFoundError;
///
/// assert_eq!(ToolNotFoundError::new("weather").to_string(), "Tool 'weather' not found");
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
#[error("Tool '{tool_name}' not found")]
pub struct ToolNotFoundError {
    tool_name: String,
}

impl ToolNotFoundError {
    /// The TS `error.name`, `"ToolNotFoundError"`.
    pub const NAME: &'static str = "ToolNotFoundError";

    /// Creates the error for the missing tool.
    #[must_use]
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
        }
    }

    /// The name that was looked up.
    #[must_use]
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

/// Wire data failed validation, for example an unknown content block in strict data, or a required
/// field was missing.
///
/// TS throws a plain `Error` in these cases, so the name is `"Error"`; the message is the TS one.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct DataError {
    message: String,
}

impl DataError {
    /// The TS `error.name` of a plain `Error`, `"Error"`.
    pub const NAME: &'static str = "Error";

    /// Creates a data error with the given message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }

    /// The error message.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }
}

/// An I/O or JSON failure the SDK passes through unchanged.
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
pub enum OtherError {
    /// An I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A JSON encoding or decoding failure.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

impl OtherError {
    /// The TS `error.name` of a plain `Error`, `"Error"`.
    pub const NAME: &'static str = "Error";
}

#[cfg(test)]
mod tests;
