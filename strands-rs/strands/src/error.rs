//! Errors returned by the SDK (TS `src/errors.ts`, `src/sandbox/errors.ts`).
//!
//! Each TS error class is a struct with the TS message text and a `NAME` constant holding the TS
//! `error.name`; [`Error`] has one variant per class.

use std::time::Duration;

use crate::types::messages::Message;

/// A boxed error, used for error sources and plumbing failures.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// The result type used throughout the SDK.
pub type Result<T, E = Error> = std::result::Result<T, E>;

macro_rules! message_error {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, thiserror::Error)]
        #[error("{message}")]
        #[non_exhaustive]
        pub struct $name {
            message: String,
        }

        impl $name {
            /// The TS `error.name`.
            pub const NAME: &'static str = stringify!($name);

            /// An error with this message.
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into() }
            }

            /// The message.
            pub fn message(&self) -> &str {
                &self.message
            }
        }
    };
}

macro_rules! sourced_error {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, thiserror::Error)]
        #[error("{message}")]
        #[non_exhaustive]
        pub struct $name {
            message: String,
            #[source]
            source: Option<BoxError>,
        }

        impl $name {
            /// The TS `error.name`.
            pub const NAME: &'static str = stringify!($name);

            /// An error with this message.
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into(), source: None }
            }

            /// Sets the underlying cause (TS `cause`).
            #[must_use]
            pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
                self.source = Some(source.into());
                self
            }

            /// The message.
            pub fn message(&self) -> &str {
                &self.message
            }
        }
    };
}

macro_rules! path_error {
    ($(#[$meta:meta])* $name:ident, $prefix:literal, $code:literal) => {
        $(#[$meta])*
        #[derive(Debug, thiserror::Error)]
        #[error("{prefix}: {path}", prefix = $prefix)]
        #[non_exhaustive]
        pub struct $name {
            path: String,
            #[source]
            source: Option<BoxError>,
        }

        impl $name {
            /// The TS `error.name`.
            pub const NAME: &'static str = stringify!($name);
            /// The Node `fs`-style error code.
            pub const CODE: &'static str = $code;

            /// An error for this path.
            pub fn new(path: impl Into<String>) -> Self {
                Self { path: path.into(), source: None }
            }

            /// Sets the underlying cause (TS `cause`).
            #[must_use]
            pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
                self.source = Some(source.into());
                self
            }

            /// The path.
            pub fn path(&self) -> &str {
                &self.path
            }

            /// The Node `fs`-style error code.
            pub fn code(&self) -> &'static str {
                Self::CODE
            }
        }
    };
}

sourced_error! {
    /// A model provider failed (TS `ModelError`); the base of the model error family.
    ModelError
}

message_error! {
    /// The request exceeded the model's context window (TS `ContextWindowOverflowError`).
    ContextWindowOverflowError
}

sourced_error! {
    /// The provider throttled the request (TS `ModelThrottledError`); retried by default.
    ModelThrottledError
}

sourced_error! {
    /// The request to the provider failed in transport (TS `ModelNetworkError`); retried by default.
    ModelNetworkError
}

message_error! {
    /// A value could not be represented as JSON (TS `JsonValidationError`).
    JsonValidationError
}

message_error! {
    /// Tool input failed schema validation (TS `ToolValidationError`).
    ToolValidationError
}

message_error! {
    /// Structured output could not be produced (TS `StructuredOutputError`).
    StructuredOutputError
}

message_error! {
    /// A default (such as the default model) is not available (TS `DefaultNotConfiguredError`).
    DefaultNotConfiguredError
}

sourced_error! {
    /// A storage backend failed (TS `StorageError`).
    StorageError
}

/// The model stopped at its output token limit (TS `MaxTokensError`), with the partial message.
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct MaxTokensError {
    message: String,
    partial_message: Box<Message>,
}

impl MaxTokensError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "MaxTokensError";

    /// An error carrying the message generated before the limit.
    pub fn new(message: impl Into<String>, partial_message: Message) -> Self {
        Self {
            message: message.into(),
            partial_message: Box::new(partial_message),
        }
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The message generated before the limit.
    pub fn partial_message(&self) -> &Message {
        &self.partial_message
    }
}

/// A tool name resolved to no registered tool (TS `ToolNotFoundError`).
#[derive(Debug, thiserror::Error)]
#[error("Tool '{tool_name}' not found")]
#[non_exhaustive]
pub struct ToolNotFoundError {
    tool_name: String,
}

impl ToolNotFoundError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "ToolNotFoundError";

    /// An error for this tool name.
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
        }
    }

    /// The tool name.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

/// A sandbox command ran past its timeout (TS `SandboxTimeoutError`), with the output so far.
#[derive(Debug, thiserror::Error)]
#[error("Execution timed out after {} seconds", timeout.as_secs_f64())]
#[non_exhaustive]
pub struct SandboxTimeoutError {
    timeout: Duration,
    stdout: String,
    stderr: String,
}

impl SandboxTimeoutError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "SandboxTimeoutError";

    /// An error for this timeout, with empty output.
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// Sets the output captured before the command was killed.
    #[must_use]
    pub fn with_output(mut self, stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        self.stdout = stdout.into();
        self.stderr = stderr.into();
        self
    }

    /// The timeout that was exceeded.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// Standard output captured before the kill.
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    /// Standard error captured before the kill.
    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

/// A sandbox command was aborted (TS `SandboxAbortError`).
#[derive(Debug, Default, thiserror::Error)]
#[error("Execution aborted")]
#[non_exhaustive]
pub struct SandboxAbortError {}

impl SandboxAbortError {
    /// The TS `error.name`.
    pub const NAME: &'static str = "SandboxAbortError";

    /// The abort error.
    pub fn new() -> Self {
        Self {}
    }
}

path_error! {
    /// A sandbox path does not exist (TS `SandboxPathNotFoundError`).
    SandboxPathNotFoundError, "Path not found", "ENOENT"
}

path_error! {
    /// A sandbox file operation hit a directory (TS `SandboxIsADirectoryError`).
    SandboxIsADirectoryError, "Is a directory", "EISDIR"
}

path_error! {
    /// A sandbox file operation was denied (TS `SandboxPermissionError`).
    SandboxPermissionError, "Permission denied", "EACCES"
}

/// Every error the SDK returns, one variant per TS error class.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// [`ModelError`]
    #[error(transparent)]
    Model(#[from] ModelError),
    /// [`ContextWindowOverflowError`]
    #[error(transparent)]
    ContextWindowOverflow(#[from] ContextWindowOverflowError),
    /// [`MaxTokensError`]
    #[error(transparent)]
    MaxTokens(#[from] MaxTokensError),
    /// [`ModelThrottledError`]
    #[error(transparent)]
    ModelThrottled(#[from] ModelThrottledError),
    /// [`ModelNetworkError`]
    #[error(transparent)]
    ModelNetwork(#[from] ModelNetworkError),
    /// [`JsonValidationError`]
    #[error(transparent)]
    JsonValidation(#[from] JsonValidationError),
    /// [`ToolValidationError`]
    #[error(transparent)]
    ToolValidation(#[from] ToolValidationError),
    /// [`ToolNotFoundError`]
    #[error(transparent)]
    ToolNotFound(#[from] ToolNotFoundError),
    /// [`StructuredOutputError`]
    #[error(transparent)]
    StructuredOutput(#[from] StructuredOutputError),
    /// [`DefaultNotConfiguredError`]
    #[error(transparent)]
    DefaultNotConfigured(#[from] DefaultNotConfiguredError),
    /// [`StorageError`]
    #[error(transparent)]
    Storage(#[from] StorageError),
    /// [`SandboxTimeoutError`]
    #[error(transparent)]
    SandboxTimeout(#[from] SandboxTimeoutError),
    /// [`SandboxAbortError`]
    #[error(transparent)]
    SandboxAbort(#[from] SandboxAbortError),
    /// [`SandboxPathNotFoundError`]
    #[error(transparent)]
    SandboxPathNotFound(#[from] SandboxPathNotFoundError),
    /// [`SandboxIsADirectoryError`]
    #[error(transparent)]
    SandboxIsADirectory(#[from] SandboxIsADirectoryError),
    /// [`SandboxPermissionError`]
    #[error(transparent)]
    SandboxPermission(#[from] SandboxPermissionError),
    /// Plumbing failures (I/O, JSON) and the plain `Error` TS throws for malformed data.
    #[error("{0}")]
    Other(BoxError),
}

impl Error {
    /// Wraps a plumbing error or a message as [`Error::Other`].
    pub fn other(error: impl Into<BoxError>) -> Self {
        Self::Other(error.into())
    }

    /// The TS `error.name` of this error; `"Error"` for [`Error::Other`].
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
            Self::Other(_) => "Error",
        }
    }

    /// Whether this is in the TS `ModelError` family (`instanceof ModelError`).
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

impl From<serde_json::Error> for Error {
    fn from(error: serde_json::Error) -> Self {
        Self::Other(Box::new(error))
    }
}

impl From<std::io::Error> for Error {
    fn from(error: std::io::Error) -> Self {
        Self::Other(Box::new(error))
    }
}

#[cfg(test)]
mod tests;
