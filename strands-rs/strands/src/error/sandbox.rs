//! Sandbox errors (TS `sandbox/errors.ts`).

use std::time::Duration;

use super::Source;

/// A sandboxed command ran past its timeout and was killed (TS `SandboxTimeoutError`). Carries the
/// output captured before the kill.
///
/// ```
/// use std::time::Duration;
/// use strands::error::SandboxTimeoutError;
///
/// let error = SandboxTimeoutError::new(Duration::from_secs(30)).with_stdout("so far");
/// assert_eq!(error.to_string(), "Execution timed out after 30 seconds");
/// assert_eq!(error.stdout(), "so far");
/// ```
#[non_exhaustive]
#[derive(Debug, thiserror::Error)]
#[error("Execution timed out after {} seconds", .timeout.as_secs_f64())]
pub struct SandboxTimeoutError {
    timeout: Duration,
    stdout: String,
    stderr: String,
}

impl SandboxTimeoutError {
    /// The TS `error.name`, `"SandboxTimeoutError"`.
    pub const NAME: &'static str = "SandboxTimeoutError";

    /// Creates the error for a command killed after `timeout`, with empty output.
    #[must_use]
    pub fn new(timeout: Duration) -> Self {
        Self {
            timeout,
            stdout: String::new(),
            stderr: String::new(),
        }
    }

    /// Sets the standard output captured before the kill.
    #[must_use]
    pub fn with_stdout(mut self, stdout: impl Into<String>) -> Self {
        self.stdout = stdout.into();
        self
    }

    /// Sets the standard error captured before the kill.
    #[must_use]
    pub fn with_stderr(mut self, stderr: impl Into<String>) -> Self {
        self.stderr = stderr.into();
        self
    }

    /// The timeout the command exceeded.
    #[must_use]
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    /// The standard output captured before the kill.
    #[must_use]
    pub fn stdout(&self) -> &str {
        &self.stdout
    }

    /// The standard error captured before the kill.
    #[must_use]
    pub fn stderr(&self) -> &str {
        &self.stderr
    }
}

/// A sandboxed command was cancelled (TS `SandboxAbortError`).
#[non_exhaustive]
#[derive(Debug, Default, thiserror::Error)]
#[error("Execution aborted")]
pub struct SandboxAbortError;

impl SandboxAbortError {
    /// The TS `error.name`, `"SandboxAbortError"`.
    pub const NAME: &'static str = "SandboxAbortError";

    /// Creates the error.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// Defines a sandbox file error that names a path and carries a Node `fs`-style code.
macro_rules! path_error {
    ($(#[$meta:meta])* $name:ident, $prefix:literal, $code:literal) => {
        $(#[$meta])*
        #[non_exhaustive]
        #[derive(Debug, thiserror::Error)]
        #[error("{}: {}", $prefix, .path)]
        pub struct $name {
            path: String,
            source: Option<Source>,
        }

        impl $name {
            #[doc = concat!("The TS `error.name`, `\"", stringify!($name), "\"`.")]
            pub const NAME: &'static str = stringify!($name);

            #[doc = concat!("The Node `fs`-style error code, `\"", $code, "\"`.")]
            pub const CODE: &'static str = $code;

            /// Creates the error for `path`.
            #[must_use]
            pub fn new(path: impl Into<String>) -> Self {
                Self { path: path.into(), source: None }
            }

            /// Sets the underlying error (TS `cause`), returned by
            /// [`source`](std::error::Error::source).
            #[must_use]
            pub fn with_source(mut self, source: impl Into<Box<dyn std::error::Error + Send + Sync>>) -> Self {
                self.source = Some(source.into());
                self
            }

            /// The path the operation failed on.
            #[must_use]
            pub fn path(&self) -> &str {
                &self.path
            }

            #[doc = concat!("The Node `fs`-style error code (TS `code`), `\"", $code, "\"`.")]
            #[must_use]
            pub fn code(&self) -> &'static str {
                Self::CODE
            }
        }
    };
}

path_error!(
    /// A sandbox file operation named a path that doesn't exist (TS `SandboxPathNotFoundError`).
    SandboxPathNotFoundError,
    "Path not found",
    "ENOENT"
);

path_error!(
    /// A sandbox file operation expected a file but found a directory (TS
    /// `SandboxIsADirectoryError`).
    SandboxIsADirectoryError,
    "Is a directory",
    "EISDIR"
);

path_error!(
    /// A sandbox file operation was denied access to a path (TS `SandboxPermissionError`).
    SandboxPermissionError,
    "Permission denied",
    "EACCES"
);

#[cfg(test)]
mod tests {
    //! Ported from strands-ts/src/sandbox/__tests__/errors.test.node.ts

    use super::*;
    use crate::error::Error;

    fn timeout_error() -> SandboxTimeoutError {
        SandboxTimeoutError::new(Duration::from_secs(30))
    }

    mod sandbox_timeout_error {
        use super::*;

        #[test]
        fn builds_a_message_from_the_timeout_duration() {
            assert_eq!(
                timeout_error().to_string(),
                "Execution timed out after 30 seconds"
            );
        }

        #[test]
        fn has_the_correct_error_name() {
            assert_eq!(Error::from(timeout_error()).name(), "SandboxTimeoutError");
        }

        #[test]
        fn is_an_instance_of_error() {
            let error = Error::from(timeout_error());
            assert!(matches!(error, Error::SandboxTimeout(_)));
            assert_eq!(error.to_string(), "Execution timed out after 30 seconds");
        }

        #[test]
        fn carries_the_partial_output_captured_before_the_kill() {
            let error = timeout_error().with_stdout("so far").with_stderr("warn");
            assert_eq!(error.stdout(), "so far");
            assert_eq!(error.stderr(), "warn");
        }

        #[test]
        fn defaults_to_empty_output_when_no_partial_output_is_given() {
            let error = timeout_error();
            assert_eq!(error.stdout(), "");
            assert_eq!(error.stderr(), "");
        }

        #[test]
        fn formats_fractional_seconds_as_ts_does() {
            let error = SandboxTimeoutError::new(Duration::from_millis(1500));
            assert_eq!(error.to_string(), "Execution timed out after 1.5 seconds");
            assert_eq!(error.timeout(), Duration::from_millis(1500));
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
            let error = Error::from(SandboxAbortError::new());
            assert!(matches!(error, Error::SandboxAbort(_)));
            assert_eq!(error.to_string(), "Execution aborted");
        }
    }

    fn code(error: &Error) -> &'static str {
        match error {
            Error::SandboxPathNotFound(error) => error.code(),
            Error::SandboxIsADirectory(error) => error.code(),
            Error::SandboxPermission(error) => error.code(),
            other => panic!("not a sandbox path error: {other:?}"),
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
        for (error, name, message, expected_code) in cases {
            assert_eq!(error.name(), name);
            assert_eq!(error.to_string(), message);
            assert_eq!(code(&error), expected_code);
        }
    }

    #[test]
    fn path_errors_keep_the_path_and_the_cause() {
        let cause = std::io::Error::from(std::io::ErrorKind::NotFound);
        let error = SandboxPathNotFoundError::new("a.txt").with_source(cause);
        assert_eq!(error.path(), "a.txt");
        let source = std::error::Error::source(&error).expect("a source");
        assert!(source.downcast_ref::<std::io::Error>().is_some());
    }
}
