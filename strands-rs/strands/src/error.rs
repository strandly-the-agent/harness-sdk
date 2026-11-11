//! Errors (TS `errors.ts`).
//!
//! Each TS error class is a struct with the same name and message; [`Error`] has one variant per
//! class. Interrupts are not errors: they are tool outcomes and agent results.

use crate::sandbox::{
    SandboxAbortError, SandboxIsADirectoryError, SandboxPathNotFoundError, SandboxPermissionError,
    SandboxTimeoutError,
};
use crate::types::messages::Message;

/// A boxed error used as a `source`.
pub type BoxError = Box<dyn std::error::Error + Send + Sync + 'static>;

/// `Result` with [`Error`] as the default error type.
pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Defines an error class with a message and, where TS takes a `cause`, an optional source.
macro_rules! error_class {
    ($(#[$meta:meta])* $name:ident, source) => {
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
            /// An error with `message`.
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into(), source: None }
            }

            /// Sets the underlying cause.
            #[must_use]
            pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
                self.source = Some(source.into());
                self
            }
        }

        error_class!(@common $name);
    };
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(Debug, thiserror::Error)]
        #[error("{message}")]
        #[non_exhaustive]
        pub struct $name {
            message: String,
        }

        impl $name {
            /// An error with `message`.
            pub fn new(message: impl Into<String>) -> Self {
                Self { message: message.into() }
            }
        }

        error_class!(@common $name);
    };
    (@common $name:ident) => {
        impl $name {
            #[doc = concat!("The TS `error.name`: `", stringify!($name), "`.")]
            pub const NAME: &'static str = stringify!($name);

            /// The error message.
            pub fn message(&self) -> &str {
                &self.message
            }
        }
    };
}

error_class!(
    /// A model provider failed (TS `ModelError`). Base of the model error family.
    ModelError, source
);
error_class!(
    /// The request exceeded the model's context window (TS `ContextWindowOverflowError`).
    ContextWindowOverflowError
);
error_class!(
    /// The provider rate-limited the request (TS `ModelThrottledError`).
    ModelThrottledError, source
);
error_class!(
    /// The connection to the provider failed (TS `ModelNetworkError`).
    ModelNetworkError, source
);
error_class!(
    /// A value failed JSON validation (TS `JsonValidationError`).
    JsonValidationError
);
error_class!(
    /// Tool input failed validation against the tool's schema (TS `ToolValidationError`).
    ToolValidationError
);
error_class!(
    /// The model did not produce valid structured output (TS `StructuredOutputError`).
    StructuredOutputError
);
error_class!(
    /// A default the call relies on is not configured (TS `DefaultNotConfiguredError`).
    DefaultNotConfiguredError
);
error_class!(
    /// A storage backend failed (TS `StorageError`).
    StorageError, source
);
error_class!(
    /// A session could not be saved or restored (TS `SessionError`).
    SessionError, source
);

/// The model hit its token limit before finishing (TS `MaxTokensError`).
#[derive(Debug, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct MaxTokensError {
    message: String,
    partial_message: Box<Message>,
}

impl MaxTokensError {
    /// The TS `error.name`: `MaxTokensError`.
    pub const NAME: &'static str = "MaxTokensError";

    /// An error carrying the partial message generated before the limit.
    pub fn new(message: impl Into<String>, partial_message: Message) -> Self {
        Self {
            message: message.into(),
            partial_message: Box::new(partial_message),
        }
    }

    /// The error message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// What the model generated before the limit.
    pub fn partial_message(&self) -> &Message {
        &self.partial_message
    }
}

/// No tool with the requested name is registered (TS `ToolNotFoundError`).
#[derive(Debug, thiserror::Error)]
#[error("Tool '{tool_name}' not found")]
#[non_exhaustive]
pub struct ToolNotFoundError {
    tool_name: String,
}

impl ToolNotFoundError {
    /// The TS `error.name`: `ToolNotFoundError`.
    pub const NAME: &'static str = "ToolNotFoundError";

    /// An error for `tool_name`.
    pub fn new(tool_name: impl Into<String>) -> Self {
        Self {
            tool_name: tool_name.into(),
        }
    }

    /// The name that was not found.
    pub fn tool_name(&self) -> &str {
        &self.tool_name
    }
}

/// Any error the SDK returns.
///
/// One variant per TS error class, plus [`Error::Json`] and [`Error::Io`] for serialization and
/// I/O plumbing.
///
/// ```
/// use strands::error::{ContextWindowOverflowError, Error};
///
/// let err = Error::from(ContextWindowOverflowError::new("too long"));
/// assert_eq!(err.name(), "ContextWindowOverflowError");
/// assert!(err.is_model_error());
/// assert_eq!(err.to_string(), "too long");
/// ```
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
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
    /// See [`SessionError`].
    #[error(transparent)]
    Session(#[from] SessionError),
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
    /// JSON that could not be parsed or serialized.
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    /// An I/O failure.
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl Error {
    /// The TS `error.name` of this error; `Error` for [`Error::Json`] and [`Error::Io`].
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
            Self::Session(_) => SessionError::NAME,
            Self::SandboxTimeout(_) => SandboxTimeoutError::NAME,
            Self::SandboxAbort(_) => SandboxAbortError::NAME,
            Self::SandboxPathNotFound(_) => SandboxPathNotFoundError::NAME,
            Self::SandboxIsADirectory(_) => SandboxIsADirectoryError::NAME,
            Self::SandboxPermission(_) => SandboxPermissionError::NAME,
            Self::Json(_) | Self::Io(_) => "Error",
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

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/__tests__/errors.test.ts
    use std::error::Error as _;

    use super::*;
    use crate::types::{Message, Role};

    fn partial_message() -> Message {
        Message::new(Role::Assistant, vec!["partial response".into()])
    }

    fn cause() -> std::io::Error {
        std::io::Error::other("original error")
    }

    mod model_error {
        use super::*;

        #[test]
        fn creates_an_error_with_the_correct_message() {
            assert_eq!(ModelError::new("Model failed").to_string(), "Model failed");
        }

        #[test]
        fn has_the_correct_error_name() {
            assert_eq!(Error::from(ModelError::new("test")).name(), "ModelError");
        }

        #[test]
        fn is_an_instance_of_error() {
            assert!(Error::from(ModelError::new("test")).is_model_error());
        }

        #[test]
        fn stores_the_cause_error() {
            let error = ModelError::new("wrapped error").with_source(cause());
            assert_eq!(error.message(), "wrapped error");
            assert_eq!(error.source().unwrap().to_string(), "original error");
        }
    }

    mod context_window_overflow_error {
        use super::*;

        #[test]
        fn creates_an_error_with_the_correct_message() {
            assert_eq!(
                ContextWindowOverflowError::new("Context full").to_string(),
                "Context full"
            );
        }

        #[test]
        fn has_the_correct_error_name() {
            assert_eq!(
                Error::from(ContextWindowOverflowError::new("test")).name(),
                "ContextWindowOverflowError"
            );
        }

        #[test]
        fn is_an_instance_of_error() {
            let _: &dyn std::error::Error = &ContextWindowOverflowError::new("test");
        }

        #[test]
        fn is_an_instance_of_model_error() {
            assert!(Error::from(ContextWindowOverflowError::new("test")).is_model_error());
        }
    }

    mod max_tokens_error {
        use super::*;

        #[test]
        fn creates_an_error_with_the_correct_message() {
            assert_eq!(
                MaxTokensError::new("Max tokens reached", partial_message()).to_string(),
                "Max tokens reached"
            );
        }

        #[test]
        fn has_the_correct_error_name() {
            assert_eq!(
                Error::from(MaxTokensError::new("test", partial_message())).name(),
                "MaxTokensError"
            );
        }

        #[test]
        fn stores_the_partial_message() {
            let message = partial_message();
            assert_eq!(
                MaxTokensError::new("Max tokens reached", message.clone()).partial_message(),
                &message
            );
        }

        #[test]
        fn is_an_instance_of_error() {
            let _: &dyn std::error::Error = &MaxTokensError::new("test", partial_message());
        }

        #[test]
        fn is_an_instance_of_model_error() {
            assert!(Error::from(MaxTokensError::new("test", partial_message())).is_model_error());
        }
    }

    mod model_throttled_error {
        use super::*;

        #[test]
        fn creates_an_error_with_the_correct_message() {
            assert_eq!(
                ModelThrottledError::new("Rate limit exceeded").to_string(),
                "Rate limit exceeded"
            );
        }

        #[test]
        fn has_the_correct_error_name() {
            assert_eq!(
                Error::from(ModelThrottledError::new("test")).name(),
                "ModelThrottledError"
            );
        }

        #[test]
        fn is_an_instance_of_error() {
            let _: &dyn std::error::Error = &ModelThrottledError::new("test");
        }

        #[test]
        fn is_an_instance_of_model_error() {
            assert!(Error::from(ModelThrottledError::new("test")).is_model_error());
        }

        #[test]
        fn preserves_the_original_error_as_cause() {
            let error = ModelThrottledError::new("Rate limit exceeded").with_source(cause());
            assert_eq!(error.source().unwrap().to_string(), "original error");
        }

        #[test]
        fn has_undefined_cause_when_not_provided() {
            assert!(ModelThrottledError::new("Rate limit exceeded").source().is_none());
        }
    }

    #[test]
    fn is_a_model_error_with_its_own_name_that_preserves_the_cause() {
        let error = Error::from(
            ModelNetworkError::new("Language model stream error: Failed to fetch")
                .with_source(std::io::Error::other("Failed to fetch")),
        );
        assert!(error.is_model_error());
        assert_eq!(error.name(), "ModelNetworkError");
        assert_eq!(error.to_string(), "Language model stream error: Failed to fetch");
        assert_eq!(error.source().unwrap().to_string(), "Failed to fetch");
    }

    #[test]
    fn every_class_reports_its_ts_name() {
        let cases: Vec<(Error, &str)> = vec![
            (JsonValidationError::new("m").into(), "JsonValidationError"),
            (ToolValidationError::new("m").into(), "ToolValidationError"),
            (ToolNotFoundError::new("m").into(), "ToolNotFoundError"),
            (StructuredOutputError::new("m").into(), "StructuredOutputError"),
            (
                DefaultNotConfiguredError::new("m").into(),
                "DefaultNotConfiguredError",
            ),
            (StorageError::new("m").into(), "StorageError"),
            (SessionError::new("m").into(), "SessionError"),
        ];
        for (error, name) in cases {
            assert_eq!(error.name(), name);
            assert!(!error.is_model_error(), "{name}");
        }
    }

    #[test]
    fn tool_not_found_error_names_the_tool() {
        let error = ToolNotFoundError::new("get_weather");
        assert_eq!(error.to_string(), "Tool 'get_weather' not found");
        assert_eq!(error.tool_name(), "get_weather");
    }

    #[test]
    fn error_is_send_and_sync() {
        fn assert_send_sync<T: Send + Sync>(_: &T) {}
        assert_send_sync(&Error::from(ModelError::new("m")));
    }
}
