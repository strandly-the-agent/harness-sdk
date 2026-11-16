//! The sandbox abstraction (TS `sandbox/base.ts`).

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;

use crate::cancel::CancelSignal;
use crate::error::{Error, Result};
use crate::models::model::BoxStream;
use crate::tools::Tool;

use super::types::{ExecutionEvent, ExecutionResult, FileInfo};

/// Options for command and code execution (TS `ExecuteOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, Default, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"), derive(Debug))]
pub struct ExecuteOptions {
    /// Maximum execution time; `None` means no timeout.
    pub timeout: Option<Duration>,
    /// Working directory; `None` means the sandbox default.
    pub cwd: Option<String>,
    /// Cancels the execution; the process is killed when it fires (TS `signal`).
    pub cancel_signal: Option<CancelSignal>,
    /// Environment variables for this command. Built-in sandboxes always apply them; a custom
    /// [`Sandbox`] must handle them explicitly or they have no effect.
    pub env: Option<BTreeMap<String, String>>,
}

/// An execution environment for commands, code and file operations (TS `Sandbox`).
///
/// Tools that share a sandbox share its working directory and filesystem. The streaming methods
/// are the primitives; [`execute`](Self::execute) and [`execute_code`](Self::execute_code)
/// consume them and return the final result. Implement [`PosixShellSandbox`] instead when only a
/// shell is available: it derives everything else from
/// [`execute_streaming`](super::PosixShellSandbox::execute_streaming).
///
/// [`PosixShellSandbox`]: super::PosixShellSandbox
#[async_trait]
pub trait Sandbox: Send + Sync + 'static {
    /// Runs a shell command, yielding output chunks as they arrive and then one
    /// [`ExecutionEvent::ExecutionResult`].
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>>;

    /// Runs `code` through the interpreter `language` (for example `python3`), streaming like
    /// [`execute_streaming`](Self::execute_streaming).
    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>>;

    /// Reads a file as raw bytes.
    ///
    /// # Errors
    ///
    /// [`Error::SandboxPathNotFound`] when the path doesn't exist, [`Error::SandboxIsADirectory`]
    /// when it is a directory, [`Error::SandboxPermission`] when it can't be read, and any other
    /// failure of the backend.
    async fn read_file(&self, path: &str) -> Result<Vec<u8>>;

    /// Writes raw bytes to a file, creating missing parent directories.
    ///
    /// # Errors
    ///
    /// The backend failed to write the file.
    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()>;

    /// Removes a file.
    ///
    /// # Errors
    ///
    /// The file doesn't exist or can't be removed.
    async fn remove_file(&self, path: &str) -> Result<()>;

    /// Lists a directory.
    ///
    /// # Errors
    ///
    /// [`Error::SandboxPathNotFound`] when the directory doesn't exist, and any other failure of
    /// the backend.
    async fn list_files(&self, path: &str) -> Result<Vec<FileInfo>>;

    /// Tools this sandbox vends to an agent, registered by `Agent::initialize`; a tool is skipped
    /// when the agent already has one with the same name. None by default.
    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }

    /// Runs a shell command and returns its final result.
    ///
    /// # Errors
    ///
    /// The execution failed (for example [`Error::SandboxTimeout`] or [`Error::SandboxAbort`]),
    /// or the stream ended without a result.
    async fn execute(&self, command: &str, options: ExecuteOptions) -> Result<ExecutionResult> {
        final_result(self.execute_streaming(command, options), "executeStreaming").await
    }

    /// Runs `code` through `language` and returns its final result.
    ///
    /// # Errors
    ///
    /// As [`execute`](Self::execute).
    async fn execute_code(
        &self,
        code: &str,
        language: &str,
        options: ExecuteOptions,
    ) -> Result<ExecutionResult> {
        final_result(
            self.execute_code_streaming(code, language, options),
            "executeCodeStreaming",
        )
        .await
    }

    /// Reads a file as UTF-8 text; invalid sequences become U+FFFD (TS `TextDecoder`).
    ///
    /// # Errors
    ///
    /// As [`read_file`](Self::read_file).
    async fn read_text(&self, path: &str) -> Result<String> {
        let bytes = self.read_file(path).await?;
        Ok(String::from_utf8(bytes)
            .unwrap_or_else(|error| String::from_utf8_lossy(error.as_bytes()).into_owned()))
    }

    /// Writes text to a file as UTF-8.
    ///
    /// # Errors
    ///
    /// As [`write_file`](Self::write_file).
    async fn write_text(&self, path: &str, content: &str) -> Result<()> {
        self.write_file(path, content.as_bytes()).await
    }
}

impl fmt::Debug for dyn Sandbox {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Sandbox").finish_non_exhaustive()
    }
}

pub(crate) async fn final_result(
    mut stream: BoxStream<'_, Result<ExecutionEvent>>,
    method: &str,
) -> Result<ExecutionResult> {
    while let Some(event) = stream.next().await {
        if let ExecutionEvent::ExecutionResult(result) = event? {
            return Ok(result);
        }
    }
    Err(Error::other(format!(
        "{method}() did not yield an ExecutionResult"
    )))
}
