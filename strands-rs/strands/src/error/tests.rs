//! Ported from strands-ts/src/__tests__/errors.test.ts

use std::error::Error as StdError;

use super::*;
use crate::types::messages::{ContentBlock, Role};

#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct Cause(&'static str);

fn source_of(error: &dyn StdError) -> Option<&Cause> {
    error.source()?.downcast_ref::<Cause>()
}

fn partial_message() -> Message {
    Message::new(Role::Assistant, vec![ContentBlock::from("partial response")])
}

mod model_error {
    use super::*;

    #[test]
    fn creates_an_error_with_the_correct_message() {
        let error = ModelError::new("Model error occurred");
        assert_eq!(error.message(), "Model error occurred");
        assert_eq!(Error::from(error).to_string(), "Model error occurred");
    }

    #[test]
    fn has_the_correct_error_name() {
        assert_eq!(Error::from(ModelError::new("test")).name(), "ModelError");
    }

    #[test]
    fn is_an_instance_of_error() {
        let error = Error::from(ModelError::new("test"));
        assert!(matches!(error, Error::Model(_)));
        assert!(error.is_model_error());
    }

    #[test]
    fn stores_the_cause_error() {
        let error = ModelError::new("wrapped error").with_source(Cause("original error"));
        assert_eq!(error.message(), "wrapped error");
        assert_eq!(source_of(&error).unwrap().0, "original error");
        assert_eq!(source_of(&Error::from(error)).unwrap().0, "original error");
    }
}

mod context_window_overflow_error {
    use super::*;

    #[test]
    fn creates_an_error_with_the_correct_message() {
        let error = ContextWindowOverflowError::new("Context window overflow occurred");
        assert_eq!(error.message(), "Context window overflow occurred");
        assert_eq!(Error::from(error).to_string(), "Context window overflow occurred");
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
        let error = Error::from(ContextWindowOverflowError::new("test"));
        assert!(matches!(error, Error::ContextWindowOverflow(_)));
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
        let error = MaxTokensError::new("Max tokens reached", partial_message());
        assert_eq!(error.message(), "Max tokens reached");
        assert_eq!(Error::from(error).to_string(), "Max tokens reached");
    }

    #[test]
    fn has_the_correct_error_name() {
        let error = Error::from(MaxTokensError::new("test", partial_message()));
        assert_eq!(error.name(), "MaxTokensError");
    }

    #[test]
    fn stores_the_partial_message() {
        let partial = partial_message();
        let error = MaxTokensError::new("Max tokens reached", partial.clone());
        assert_eq!(error.partial_message(), &partial);
    }

    #[test]
    fn is_an_instance_of_error() {
        let error = Error::from(MaxTokensError::new("test", partial_message()));
        assert!(matches!(error, Error::MaxTokens(_)));
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
        let error = ModelThrottledError::new("Rate limit exceeded");
        assert_eq!(error.message(), "Rate limit exceeded");
        assert_eq!(Error::from(error).to_string(), "Rate limit exceeded");
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
        let error = Error::from(ModelThrottledError::new("test"));
        assert!(matches!(error, Error::ModelThrottled(_)));
    }

    #[test]
    fn is_an_instance_of_model_error() {
        assert!(Error::from(ModelThrottledError::new("test")).is_model_error());
    }

    #[test]
    fn preserves_the_original_error_as_cause() {
        let error =
            ModelThrottledError::new("Rate limit exceeded").with_source(Cause("Original rate limit error"));
        assert_eq!(source_of(&error).unwrap().0, "Original rate limit error");
    }

    #[test]
    fn has_undefined_cause_when_not_provided() {
        assert!(ModelThrottledError::new("Rate limit exceeded").source().is_none());
    }
}

mod model_network_error {
    use super::*;

    #[test]
    fn is_a_model_error_with_its_own_name_that_preserves_the_cause() {
        let error = Error::from(
            ModelNetworkError::new("Language model stream error: Failed to fetch")
                .with_source(Cause("Failed to fetch")),
        );
        assert!(error.is_model_error());
        assert_eq!(error.name(), "ModelNetworkError");
        assert_eq!(error.to_string(), "Language model stream error: Failed to fetch");
        assert_eq!(source_of(&error).unwrap().0, "Failed to fetch");
    }
}

#[test]
fn every_variant_has_its_ts_name_and_model_family() {
    let io = std::io::Error::other("disk");
    let json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    let cases: Vec<(Error, &str, bool)> = vec![
        (ModelError::new("m").into(), "ModelError", true),
        (
            ContextWindowOverflowError::new("m").into(),
            "ContextWindowOverflowError",
            true,
        ),
        (
            MaxTokensError::new("m", partial_message()).into(),
            "MaxTokensError",
            true,
        ),
        (ModelThrottledError::new("m").into(), "ModelThrottledError", true),
        (ModelNetworkError::new("m").into(), "ModelNetworkError", true),
        (JsonValidationError::new("m").into(), "JsonValidationError", false),
        (ToolValidationError::new("m").into(), "ToolValidationError", false),
        (ToolNotFoundError::new("m").into(), "ToolNotFoundError", false),
        (
            StructuredOutputError::new("m").into(),
            "StructuredOutputError",
            false,
        ),
        (
            DefaultNotConfiguredError::new("m").into(),
            "DefaultNotConfiguredError",
            false,
        ),
        (StorageError::new("m").into(), "StorageError", false),
        (
            SandboxTimeoutError::new(std::time::Duration::from_secs(1)).into(),
            "SandboxTimeoutError",
            false,
        ),
        (SandboxAbortError::new().into(), "SandboxAbortError", false),
        (
            SandboxPathNotFoundError::new("p").into(),
            "SandboxPathNotFoundError",
            false,
        ),
        (
            SandboxIsADirectoryError::new("p").into(),
            "SandboxIsADirectoryError",
            false,
        ),
        (
            SandboxPermissionError::new("p").into(),
            "SandboxPermissionError",
            false,
        ),
        (DataError::new("m").into(), "Error", false),
        (io.into(), "Error", false),
        (json.into(), "Error", false),
    ];
    for (error, name, is_model_error) in cases {
        assert_eq!(error.name(), name, "{error:?}");
        assert_eq!(error.is_model_error(), is_model_error, "{error:?}");
    }
}

#[test]
fn tool_not_found_error_names_the_tool() {
    let error = ToolNotFoundError::new("weather");
    assert_eq!(error.tool_name(), "weather");
    assert_eq!(Error::from(error).to_string(), "Tool 'weather' not found");
}

#[test]
fn storage_error_keeps_its_cause() {
    let error = Error::from(StorageError::new("Storage key must not be empty").with_source(Cause("io")));
    assert_eq!(error.to_string(), "Storage key must not be empty");
    assert_eq!(source_of(&error).unwrap().0, "io");
}

#[test]
fn other_errors_pass_io_and_json_failures_through() {
    let error = Error::from(std::io::Error::other("disk full"));
    assert!(matches!(error, Error::Other(OtherError::Io(_))));
    assert_eq!(error.to_string(), "disk full");
}
