//! Sandbox abstraction (`strands-ts/src/sandbox/**`): where an agent's commands and file
//! operations actually run.
//!
//! [`Sandbox`] mirrors the TS abstract class; [`LocalSandbox`] is the port of
//! `NotASandboxLocalEnvironment` — host execution with **no isolation**, the registered default
//! in Node. The Docker and SSH backends (and their `PosixShellSandbox` base) are **not ported**;
//! the trait is left open so they can be added later.

mod local;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock, RwLock};
use std::time::Duration;

use async_trait::async_trait;
use futures::stream::{self, BoxStream};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::tools::ToolRef;

pub use local::LocalSandbox;

/// Which standard stream a [`StreamChunk`] came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum StreamType {
    /// Standard output.
    Stdout,
    /// Standard error.
    Stderr,
}

/// A chunk of output produced while a command is still running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StreamChunk {
    /// Decoded text of the chunk.
    pub data: String,
    /// Stream the chunk came from.
    pub stream_type: StreamType,
}

/// One entry of a directory listing. `is_dir`/`size` are `None` when the backend cannot tell.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileInfo {
    /// Entry name (not a full path).
    pub name: String,
    /// Whether the entry is a directory.
    pub is_dir: Option<bool>,
    /// Size in bytes.
    pub size: Option<u64>,
}

/// A file produced by an execution (Jupyter/API-backed sandboxes); shell backends return none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OutputFile {
    /// File name.
    pub name: String,
    /// File content.
    pub content: Vec<u8>,
    /// MIME type.
    pub mime_type: String,
}

/// Terminal result of an execution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ExecutionResult {
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
    /// Process exit code; non-zero means the command failed.
    pub exit_code: i32,
    /// Files produced by the execution, if the backend supports them.
    pub output_files: Vec<OutputFile>,
}

/// An item yielded by [`Sandbox::execute_streaming`] (TS `StreamChunk | ExecutionResult`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SandboxEvent {
    /// Incremental output.
    Chunk(StreamChunk),
    /// The terminal result; always the last item.
    Result(ExecutionResult),
}

/// Per-call execution options. All fields are optional; the backend applies what it can.
#[derive(Clone, Debug, Default)]
pub struct ExecuteOptions {
    /// Kill the command after this long. `None` = no timeout.
    pub timeout: Option<Duration>,
    /// Working directory. `None` = the sandbox default.
    pub cwd: Option<PathBuf>,
    /// Extra environment variables.
    pub env: Option<HashMap<String, String>>,
    /// Text written to the command's stdin, then closed. `None` = stdin is `/dev/null`.
    pub stdin: Option<String>,
    /// Kills the command when cancelled (the Rust equivalent of TS `AbortSignal`).
    pub cancel: Option<tokio_util::sync::CancellationToken>,
}

impl ExecuteOptions {
    /// Options with just a timeout set.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout: Some(timeout),
            ..Self::default()
        }
    }
}

/// Why a sandbox operation failed. `Display` strings are verbatim from TS `sandbox/errors.ts`.
#[derive(Clone, Debug, thiserror::Error)]
#[non_exhaustive]
pub enum SandboxError {
    /// The command exceeded its timeout and was killed, carrying the partial output
    /// (`SandboxTimeoutError`).
    #[error("Execution timed out after {seconds} seconds")]
    Timeout {
        /// Timeout in seconds, as requested.
        seconds: f64,
        /// Standard output captured before the kill.
        stdout: String,
        /// Standard error captured before the kill.
        stderr: String,
    },
    /// The execution was cancelled (`SandboxAbortError`).
    #[error("Execution aborted")]
    Aborted,
    /// A file or directory does not exist (`SandboxPathNotFoundError`).
    #[error("Path not found: {0}")]
    PathNotFound(String),
    /// Anything else the backend reports.
    #[error("{0}")]
    Other(String),
}

impl From<SandboxError> for Error {
    fn from(error: SandboxError) -> Self {
        Error::Other(Box::new(error))
    }
}

/// Result alias for sandbox operations.
pub type SandboxResult<T> = std::result::Result<T, SandboxError>;

/// `^[a-zA-Z0-9._-]+$` — the TS `LANGUAGE_PATTERN` for `execute_code` interpreters.
pub fn is_valid_language(language: &str) -> bool {
    !language.is_empty()
        && language
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
}

/// `^[a-zA-Z_][a-zA-Z0-9_]*$` — the TS `ENV_KEY_PATTERN`.
pub fn is_valid_env_key(key: &str) -> bool {
    let mut chars = key.chars();
    match chars.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

/// Single-quote `value` for a POSIX shell (TS `shellQuote`).
pub fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Where an agent runs commands and file operations.
///
/// Deviations from TS, both noted in the module docs: `execute` is the required primitive (TS
/// makes `executeStreaming` the primitive) and the default `execute_streaming` yields only the
/// terminal result, so a backend that can stream overrides it; there is no `stat` (TS has none
/// either) and no `start`/`stop` lifecycle (TS has none).
#[async_trait]
pub trait Sandbox: std::fmt::Debug + Send + Sync {
    /// Short identifier of the backend, e.g. `"local"`.
    fn kind(&self) -> &str;

    /// Human-readable description, used in the vended shell tool's description.
    fn description(&self) -> &str {
        self.kind()
    }

    /// Run `command` and return its terminal result.
    async fn execute(
        &self,
        command: &str,
        options: ExecuteOptions,
    ) -> SandboxResult<ExecutionResult>;

    /// Run `command`, yielding output as it arrives and the terminal result last.
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, SandboxResult<SandboxEvent>> {
        Box::pin(stream::once(async move {
            self.execute(command, options).await.map(SandboxEvent::Result)
        }))
    }

    /// Run `code` through the named interpreter (`python`, `node`, …).
    async fn execute_code(
        &self,
        code: &str,
        language: &str,
        options: ExecuteOptions,
    ) -> SandboxResult<ExecutionResult> {
        if !is_valid_language(language) {
            return Err(SandboxError::Other(format!(
                "language parameter contains invalid characters: {language}"
            )));
        }
        self.execute(&format!("{language} -c {}", shell_quote(code)), options)
            .await
    }

    /// Read a file's raw bytes.
    async fn read_file(&self, path: &str) -> SandboxResult<Vec<u8>>;

    /// Write `content` to `path`, creating missing parent directories.
    async fn write_file(&self, path: &str, content: &[u8]) -> SandboxResult<()>;

    /// Delete a file.
    async fn remove_file(&self, path: &str) -> SandboxResult<()>;

    /// List a directory, sorted by name.
    async fn list_files(&self, path: &str) -> SandboxResult<Vec<FileInfo>>;

    /// Read a file as UTF-8 text.
    async fn read_text(&self, path: &str) -> SandboxResult<String> {
        let bytes = self.read_file(path).await?;
        String::from_utf8(bytes).map_err(|e| SandboxError::Other(e.to_string()))
    }

    /// Write UTF-8 text to a file.
    async fn write_text(&self, path: &str, content: &str) -> SandboxResult<()> {
        self.write_file(path, content.as_bytes()).await
    }

    /// Tools this sandbox vends to an agent (TS `getTools()`; default none).
    fn tools(&self) -> Vec<ToolRef> {
        Vec::new()
    }
}

/// A shared sandbox handle.
pub type SandboxRef = Arc<dyn Sandbox>;

/// Message of [`Error::DefaultNotConfigured`] when no sandbox is available (verbatim from TS).
pub const NO_SANDBOX_MESSAGE: &str = "No Sandbox configured. Pass a `sandbox` to the Agent to use sandbox features in this environment.";

fn slot() -> &'static RwLock<Option<SandboxRef>> {
    static SLOT: OnceLock<RwLock<Option<SandboxRef>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Register the process-wide default sandbox (TS `defaultSandbox.set`).
pub fn set_default_sandbox(sandbox: SandboxRef) {
    *slot().write().expect("default sandbox slot poisoned") = Some(sandbox);
}

/// Clear the process-wide default sandbox, making [`default_sandbox`] error again.
pub fn clear_default_sandbox() {
    *slot().write().expect("default sandbox slot poisoned") = None;
}

/// The process-wide default sandbox (TS `defaultSandbox.get`).
///
/// Errors with [`Error::DefaultNotConfigured`] when nothing is registered, exactly as the TS
/// default slot throws `DefaultNotConfiguredError`.
pub fn default_sandbox() -> Result<SandboxRef> {
    slot()
        .read()
        .expect("default sandbox slot poisoned")
        .clone()
        .ok_or_else(|| Error::DefaultNotConfigured(NO_SANDBOX_MESSAGE.to_owned()))
}

/// Register [`LocalSandbox`] as the default (the port of TS `registerNodeSandboxDefault`).
pub fn register_local_sandbox_default() {
    set_default_sandbox(Arc::new(LocalSandbox::new()));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_quote_handles_paths_with_spaces() {
        assert_eq!(shell_quote("/tmp/a b"), "'/tmp/a b'");
    }

    #[test]
    fn shell_quote_handles_paths_with_single_quotes() {
        assert_eq!(shell_quote("it's"), "'it'\\''s'");
    }

    #[test]
    fn rejects_invalid_language_characters() {
        for bad in ["../python", "py;ls", "py thon", ""] {
            assert!(!is_valid_language(bad), "{bad}");
        }
        for good in ["python3", "node", "py-thon", "py.thon"] {
            assert!(is_valid_language(good), "{good}");
        }
    }

    #[test]
    fn rejects_invalid_env_var_names() {
        for bad in ["1BAD", "BAD-KEY", "", "BAD KEY"] {
            assert!(!is_valid_env_key(bad), "{bad}");
        }
        for good in ["GOOD", "_good", "G1"] {
            assert!(is_valid_env_key(good), "{good}");
        }
    }

    #[test]
    fn timeout_error_message_is_verbatim() {
        let error = SandboxError::Timeout {
            seconds: 1.5,
            stdout: "out".to_owned(),
            stderr: String::new(),
        };
        assert_eq!(error.to_string(), "Execution timed out after 1.5 seconds");
        assert_eq!(SandboxError::Aborted.to_string(), "Execution aborted");
        assert_eq!(
            SandboxError::PathNotFound("/x".to_owned()).to_string(),
            "Path not found: /x"
        );
    }
}
