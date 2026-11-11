//! Errors raised by sandbox execution and file operations (TS `sandbox/errors.ts`).

use std::time::Duration;

use crate::error::BoxError;
use crate::types::json::js_number;

/// Execution ran past its timeout (TS `SandboxTimeoutError`).
///
/// `stdout` and `stderr` hold what the process wrote before it was killed.
#[derive(Debug, thiserror::Error)]
#[error("Execution timed out after {} seconds", js_number(.timeout.as_secs_f64()))]
#[non_exhaustive]
pub struct SandboxTimeoutError {
    timeout: Duration,
    stdout: String,
    stderr: String,
}

impl SandboxTimeoutError {
    /// The TS `error.name`: `SandboxTimeoutError`.
    pub const NAME: &'static str = "SandboxTimeoutError";

    /// A timeout error with no captured output.
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// Sets the output captured before the process was killed.
    #[must_use]
    pub fn with_output(mut self, stdout: impl Into<String>, stderr: impl Into<String>) -> Self {
        self.stdout = stdout.into();
        self.stderr = stderr.into();
        self
    }

    /// The timeout that elapsed.
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

/// Execution was aborted through its cancel signal (TS `SandboxAbortError`).
#[derive(Debug, Default, thiserror::Error)]
#[error("Execution aborted")]
#[non_exhaustive]
pub struct SandboxAbortError {}

impl SandboxAbortError {
    /// The TS `error.name`: `SandboxAbortError`.
    pub const NAME: &'static str = "SandboxAbortError";

    /// An abort error.
    pub fn new() -> Self {
        Self {}
    }
}

/// Defines a path error with a fixed message prefix and a Node fs-style `code`.
macro_rules! path_error {
    ($(#[$meta:meta])* $name:ident, $prefix:literal, $code:literal) => {
        $(#[$meta])*
        #[derive(Debug, thiserror::Error)]
        #[error("{}{path}", $prefix)]
        #[non_exhaustive]
        pub struct $name {
            path: String,
            #[source]
            source: Option<BoxError>,
        }

        impl $name {
            #[doc = concat!("The TS `error.name`: `", stringify!($name), "`.")]
            pub const NAME: &'static str = stringify!($name);
            #[doc = concat!("The Node fs-style error code: `", $code, "`.")]
            pub const CODE: &'static str = $code;

            /// An error for `path`.
            pub fn new(path: impl Into<String>) -> Self {
                Self { path: path.into(), source: None }
            }

            /// Sets the underlying cause.
            #[must_use]
            pub fn with_source(mut self, source: impl Into<BoxError>) -> Self {
                self.source = Some(source.into());
                self
            }

            /// The path the operation failed on.
            pub fn path(&self) -> &str {
                &self.path
            }

            #[doc = concat!("The Node fs-style error code, `", $code, "`.")]
            pub fn code(&self) -> &'static str {
                Self::CODE
            }
        }
    };
}

path_error!(
    /// The path does not exist (TS `SandboxPathNotFoundError`).
    SandboxPathNotFoundError, "Path not found: ", "ENOENT"
);
path_error!(
    /// The path is a directory where a file was expected (TS `SandboxIsADirectoryError`).
    SandboxIsADirectoryError, "Is a directory: ", "EISDIR"
);
path_error!(
    /// The path, or a directory above it, is not accessible (TS `SandboxPermissionError`).
    SandboxPermissionError, "Permission denied: ", "EACCES"
);

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/sandbox/__tests__/errors.test.node.ts
    use std::error::Error as _;

    use super::*;
    use crate::error::Error;

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
        let cases: [(Error, &str, &str, &str); 3] = [
            (
                SandboxPathNotFoundError::new("a.txt").into(),
                "SandboxPathNotFoundError",
                "Path not found: a.txt",
                "ENOENT",
            ),
            (
                SandboxIsADirectoryError::new("a.txt").into(),
                "SandboxIsADirectoryError",
                "Is a directory: a.txt",
                "EISDIR",
            ),
            (
                SandboxPermissionError::new("a.txt").into(),
                "SandboxPermissionError",
                "Permission denied: a.txt",
                "EACCES",
            ),
        ];
        for (error, name, message, code) in cases {
            assert_eq!(error.name(), name);
            assert_eq!(error.to_string(), message);
            let actual_code = match &error {
                Error::SandboxPathNotFound(e) => e.code(),
                Error::SandboxIsADirectory(e) => e.code(),
                Error::SandboxPermission(e) => e.code(),
                other => panic!("unexpected {other:?}"),
            };
            assert_eq!(actual_code, code);
        }
    }

    #[test]
    fn path_errors_keep_their_cause() {
        let error = SandboxPathNotFoundError::new("a.txt").with_source(std::io::Error::other("enoent"));
        assert_eq!(error.path(), "a.txt");
        assert_eq!(error.source().unwrap().to_string(), "enoent");
    }
}
