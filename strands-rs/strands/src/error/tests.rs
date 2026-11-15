//! Ported from strands-ts/src/__tests__/errors.test.ts and
//! strands-ts/src/sandbox/__tests__/errors.test.node.ts

use std::error::Error as _;
use std::time::Duration;

use super::*;
use crate::types::messages::{Role, TextBlock};

fn assert_send_sync<T: Send + Sync>() {}

fn partial_message() -> Message {
    Message::new(Role::Assistant, vec![TextBlock::new("partial response").into()])
}

fn cause() -> std::io::Error {
    std::io::Error::other("Original error")
}

#[test]
fn error_is_send_and_sync() {
    assert_send_sync::<Error>();
}

mod model_error {
    use super::*;

    #[test]
    fn creates_an_error_with_the_correct_message() {
        assert_eq!(
            ModelError::new("Test error message").to_string(),
            "Test error message"
        );
    }

    #[test]
    fn has_the_correct_error_name() {
        assert_eq!(Error::from(ModelError::new("test")).name(), "ModelError");
    }

    #[test]
    fn is_an_instance_of_error() {
        let error = Error::from(ModelError::new("test"));
        assert!(error.is_model_error());
        assert_eq!(error.to_string(), "test");
    }

    #[test]
    fn stores_the_cause_error() {
        let error = ModelError::new("Wrapped error").with_source(cause());
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("Original error")
        );
        assert_eq!(
            Error::from(error).source().map(ToString::to_string).as_deref(),
            Some("Original error")
        );
    }
}

mod context_window_overflow_error {
    use super::*;

    #[test]
    fn creates_an_error_with_the_correct_message() {
        assert_eq!(
            ContextWindowOverflowError::new("Context overflow").to_string(),
            "Context overflow"
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
        let error = MaxTokensError::new("Max tokens reached", message.clone());
        assert_eq!(error.partial_message(), &message);
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
        assert_eq!(
            error.source().map(ToString::to_string).as_deref(),
            Some("Original error")
        );
    }

    #[test]
    fn has_undefined_cause_when_not_provided() {
        assert!(ModelThrottledError::new("test").source().is_none());
    }
}

#[test]
fn is_a_model_error_with_its_own_name_that_preserves_the_cause() {
    let error = Error::from(ModelNetworkError::new("fetch failed").with_source(cause()));
    assert_eq!(error.name(), "ModelNetworkError");
    assert!(error.is_model_error());
    assert_eq!(error.to_string(), "fetch failed");
    assert_eq!(
        error.source().map(ToString::to_string).as_deref(),
        Some("Original error")
    );
}

#[test]
fn tool_not_found_error_names_the_tool() {
    let error = Error::from(ToolNotFoundError::new("calc"));
    assert_eq!(error.to_string(), "Tool 'calc' not found");
    assert_eq!(error.name(), "ToolNotFoundError");
    assert!(!error.is_model_error());
}

#[test]
fn every_class_reports_its_ts_name_and_message() {
    let cases: [(Error, &str, &str); 6] = [
        (JsonValidationError::new("m").into(), "JsonValidationError", "m"),
        (ToolValidationError::new("m").into(), "ToolValidationError", "m"),
        (
            StructuredOutputError::new("m").into(),
            "StructuredOutputError",
            "m",
        ),
        (
            DefaultNotConfiguredError::new("m").into(),
            "DefaultNotConfiguredError",
            "m",
        ),
        (
            StorageError::new("m").with_source(cause()).into(),
            "StorageError",
            "m",
        ),
        (
            Error::other("Unknown ContentBlockData type"),
            "Error",
            "Unknown ContentBlockData type",
        ),
    ];
    for (error, name, message) in cases {
        assert_eq!((error.name(), error.to_string().as_str()), (name, message));
        assert!(!error.is_model_error());
    }
}

#[test]
fn plumbing_errors_convert_to_other() {
    let json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert!(matches!(Error::from(json), Error::Other(_)));
    assert!(matches!(Error::from(cause()), Error::Other(_)));
}

#[test]
fn other_forwards_display_and_source_of_the_wrapped_error() {
    #[derive(Debug, thiserror::Error)]
    #[error("outer failure")]
    struct Outer(#[source] std::io::Error);

    let error = Error::other(Outer(cause()));
    assert_eq!(error.to_string(), "outer failure");
    assert_eq!(error.source().unwrap().to_string(), "Original error");
}

mod sandbox_timeout_error {
    use super::*;

    #[test]
    fn builds_a_message_from_the_timeout_duration() {
        assert_eq!(
            SandboxTimeoutError::new(Duration::from_secs(30)).to_string(),
            "Execution timed out after 30 seconds"
        );
        assert_eq!(
            SandboxTimeoutError::new(Duration::from_millis(1500)).to_string(),
            "Execution timed out after 1.5 seconds"
        );
    }

    #[test]
    fn has_the_correct_error_name() {
        assert_eq!(
            Error::from(SandboxTimeoutError::new(Duration::from_secs(30))).name(),
            "SandboxTimeoutError"
        );
    }

    #[test]
    fn is_an_instance_of_error() {
        let _: &dyn std::error::Error = &SandboxTimeoutError::new(Duration::from_secs(30));
    }

    #[test]
    fn carries_the_partial_output_captured_before_the_kill() {
        let error = SandboxTimeoutError::new(Duration::from_secs(30)).with_output("so far", "warn");
        assert_eq!((error.stdout(), error.stderr()), ("so far", "warn"));
    }

    #[test]
    fn defaults_to_empty_output_when_no_partial_output_is_given() {
        let error = SandboxTimeoutError::new(Duration::from_secs(30));
        assert_eq!((error.stdout(), error.stderr()), ("", ""));
        assert_eq!(error.timeout(), Duration::from_secs(30));
    }
}

mod sandbox_abort_error {
    use super::*;

    #[test]
    fn has_a_fixed_message() {
        assert_eq!(SandboxAbortError::new().to_string(), "Execution aborted");
    }

    #[test]
    fn has_the_correct_error_name() {
        assert_eq!(Error::from(SandboxAbortError::new()).name(), "SandboxAbortError");
    }

    #[test]
    fn is_an_instance_of_error() {
        let _: &dyn std::error::Error = &SandboxAbortError::new();
    }
}

#[test]
fn carries_the_path_in_its_message_and_a_node_fs_style_code() {
    let not_found = SandboxPathNotFoundError::new("a.txt");
    assert_eq!(
        (not_found.to_string().as_str(), not_found.code()),
        ("Path not found: a.txt", "ENOENT")
    );
    let directory = SandboxIsADirectoryError::new("a.txt");
    assert_eq!(
        (directory.to_string().as_str(), directory.code()),
        ("Is a directory: a.txt", "EISDIR")
    );
    let permission = SandboxPermissionError::new("a.txt").with_source(cause());
    assert_eq!(
        (permission.to_string().as_str(), permission.code()),
        ("Permission denied: a.txt", "EACCES")
    );
    assert_eq!(permission.path(), "a.txt");
    let names = [
        Error::from(not_found).name(),
        Error::from(directory).name(),
        Error::from(permission).name(),
    ];
    assert_eq!(
        names,
        [
            "SandboxPathNotFoundError",
            "SandboxIsADirectoryError",
            "SandboxPermissionError"
        ]
    );
}
