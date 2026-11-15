//! A sandbox built on a POSIX shell (TS `sandbox/posix-shell.ts`).

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::stream;

use crate::error::{
    Error, Result, SandboxIsADirectoryError, SandboxPathNotFoundError, SandboxPermissionError,
};
use crate::models::model::BoxStream;
use crate::tools::Tool;

use super::base::{ExecuteOptions, Sandbox, final_result};
use super::constants::{is_valid_env_key, is_valid_language};
use super::types::{ExecutionEvent, ExecutionResult, FileInfo};

pub use super::constants::shell_quote;

/// Exit codes the shell file commands use for typed errors (locale-independent).
const EXIT_NOT_FOUND: i32 = 77;
const EXIT_IS_A_DIRECTORY: i32 = 78;
const EXIT_PERMISSION: i32 = 79;

/// Checks every key of `env` is a POSIX environment variable name (TS `validateEnvKeys`).
///
/// # Errors
///
/// `Invalid environment variable name: <key>` for the first invalid key.
pub fn validate_env_keys(env: &BTreeMap<String, String>) -> Result<()> {
    match env.keys().find(|key| !is_valid_env_key(key)) {
        Some(key) => Err(Error::other(format!("Invalid environment variable name: {key}"))),
        None => Ok(()),
    }
}

/// The `export KEY='value' && ` prefix that applies `env` in a shell command, or `""` when
/// there is none (TS `buildShellEnvPrefix`).
///
/// `export` sets the variables in the shell itself, so every stage of a pipeline inherits them;
/// the trailing `&&` keeps a surrounding `cd ... && <prefix><command>` chain fail-fast.
///
/// # Errors
///
/// As [`validate_env_keys`].
pub fn build_shell_env_prefix(env: Option<&BTreeMap<String, String>>) -> Result<String> {
    let Some(env) = env.filter(|env| !env.is_empty()) else {
        return Ok(String::new());
    };
    validate_env_keys(env)?;
    let assignments: Vec<String> = env
        .iter()
        .map(|(key, value)| format!("{key}={}", shell_quote(value)))
        .collect();
    Ok(format!("export {} && ", assignments.join(" ")))
}

/// The `base64 -d << 'EOF' | <language>` command that runs `code` (TS `executeCodeStreaming`).
pub(crate) fn code_command(code: &str, language: &str) -> String {
    let eof = heredoc_delimiter();
    format!(
        "base64 -d << '{eof}' | {language}\n{}\n{eof}",
        STANDARD.encode(code)
    )
}

/// A here-document delimiter; base64 output never contains `_`, so it can't collide.
fn heredoc_delimiter() -> String {
    let id = uuid::Uuid::now_v7().simple().to_string();
    format!("STRANDS_EOF_{}", id.get(id.len() - 16..).unwrap_or(&id))
}

pub(crate) fn invalid_language(language: &str) -> Error {
    Error::other(format!(
        "language parameter contains invalid characters: {language}"
    ))
}

/// A sandbox where only a POSIX shell is available (TS `PosixShellSandbox`), for remote
/// environments such as containers or SSH hosts.
///
/// Implement [`execute_streaming`](Self::execute_streaming); code execution and the file
/// operations run as shell commands through it, and every type implementing this trait is a
/// [`Sandbox`]. Any provided method can be overridden with a native implementation.
/// `execute_streaming` must apply [`ExecuteOptions::env`], usually by prepending
/// [`build_shell_env_prefix`].
///
/// ```
/// use strands::models::model::BoxStream;
/// use strands::sandbox::{ExecuteOptions, ExecutionEvent, PosixShellSandbox, build_shell_env_prefix};
/// use strands::Result;
///
/// struct Echo;
///
/// impl PosixShellSandbox for Echo {
///     fn execute_streaming<'a>(
///         &'a self,
///         command: &'a str,
///         options: ExecuteOptions,
///     ) -> BoxStream<'a, Result<ExecutionEvent>> {
///         let _full = build_shell_env_prefix(options.env.as_ref()).map(|prefix| prefix + command);
///         // Send `_full` to the remote shell and stream its output back.
///         Box::pin(futures::stream::empty())
///     }
/// }
/// ```
#[async_trait]
pub trait PosixShellSandbox: Send + Sync + 'static {
    /// Runs a shell command, streaming its output and then the result (see
    /// [`Sandbox::execute_streaming`]).
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>>;

    /// Runs `code` as `base64 -d << 'EOF' | <language>` through
    /// [`execute_streaming`](Self::execute_streaming).
    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        if !is_valid_language(language) {
            return Box::pin(stream::once(async move { Err(invalid_language(language)) }));
        }
        let command = code_command(code, language);
        Box::pin(async_stream::stream! {
            let mut inner = self.execute_streaming(&command, options);
            while let Some(event) = futures::StreamExt::next(&mut inner).await {
                yield event;
            }
        })
    }

    /// Runs a shell command and returns its final result (see [`Sandbox::execute`]).
    ///
    /// # Errors
    ///
    /// As [`Sandbox::execute`].
    async fn execute(&self, command: &str, options: ExecuteOptions) -> Result<ExecutionResult> {
        final_result(self.execute_streaming(command, options), "executeStreaming").await
    }

    /// Runs `code` through `language` and returns its final result.
    ///
    /// # Errors
    ///
    /// As [`Sandbox::execute`].
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

    /// Reads a file through `base64`.
    ///
    /// # Errors
    ///
    /// As [`Sandbox::read_file`].
    async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let quoted = shell_quote(path);
        // `test -e` is also false under an unsearchable ancestor, so check the deepest existing
        // one. The braces keep a backend's `cd ... &&` prefix out of the `||`.
        let command = format!(
            "{{ test -e {quoted} || {{ p={quoted}; while p=$(dirname -- \"$p\"); ! test -d \"$p\"; do :; done; \
             test -x \"$p\" || exit 79; exit 77; }}; \
             test -r {quoted} || exit 79; test -d {quoted} && exit 78; base64 < {quoted}; }}"
        );
        let result = self.execute(&command, ExecuteOptions::default()).await?;
        match result.exit_code {
            0 => {}
            EXIT_NOT_FOUND => return Err(SandboxPathNotFoundError::new(path).into()),
            EXIT_IS_A_DIRECTORY => return Err(SandboxIsADirectoryError::new(path).into()),
            EXIT_PERMISSION => return Err(SandboxPermissionError::new(path).into()),
            _ => {
                return Err(command_error(
                    result.stderr,
                    format!("Failed to read file: {path}"),
                ));
            }
        }
        let encoded: String = result.stdout.split_whitespace().collect();
        STANDARD.decode(encoded).map_err(Error::other)
    }

    /// Writes a file through `base64 -d`, creating missing parent directories.
    ///
    /// # Errors
    ///
    /// The command failed (its stderr, or `Failed to write file: <path>`).
    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        let quoted = shell_quote(path);
        let eof = heredoc_delimiter();
        let command = format!(
            "mkdir -p \"$(dirname {quoted})\" && base64 -d << '{eof}' > {quoted}\n{}\n{eof}",
            STANDARD.encode(content)
        );
        let result = self.execute(&command, ExecuteOptions::default()).await?;
        if result.exit_code != 0 {
            return Err(command_error(
                result.stderr,
                format!("Failed to write file: {path}"),
            ));
        }
        Ok(())
    }

    /// Removes a file with `rm`.
    ///
    /// # Errors
    ///
    /// The command failed (its stderr, or `Failed to remove file: <path>`).
    async fn remove_file(&self, path: &str) -> Result<()> {
        let command = format!("rm {}", shell_quote(path));
        let result = self.execute(&command, ExecuteOptions::default()).await?;
        if result.exit_code != 0 {
            return Err(command_error(
                result.stderr,
                format!("Failed to remove file: {path}"),
            ));
        }
        Ok(())
    }

    /// Lists a directory with `ls -1ap`; sizes are unknown.
    ///
    /// # Errors
    ///
    /// [`Error::SandboxPathNotFound`] when `path` is not a directory, or the command failed.
    async fn list_files(&self, path: &str) -> Result<Vec<FileInfo>> {
        let quoted = shell_quote(path);
        let command = format!("test -d {quoted} || exit 77; env QUOTING_STYLE=literal ls -1ap {quoted}");
        let result = self.execute(&command, ExecuteOptions::default()).await?;
        if result.exit_code == EXIT_NOT_FOUND {
            return Err(SandboxPathNotFoundError::new(path).into());
        }
        if result.exit_code != 0 {
            return Err(command_error(
                result.stderr,
                format!("Failed to list directory: {path}"),
            ));
        }
        Ok(parse_listing(&result.stdout))
    }

    /// Reads a file as UTF-8 text (see [`Sandbox::read_text`]).
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

    /// Tools this sandbox vends (see [`Sandbox::get_tools`]).
    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        Vec::new()
    }
}

#[async_trait]
impl<T: PosixShellSandbox> Sandbox for T {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        PosixShellSandbox::execute_streaming(self, command, options)
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        PosixShellSandbox::execute_code_streaming(self, code, language, options)
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        PosixShellSandbox::read_file(self, path).await
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        PosixShellSandbox::write_file(self, path, content).await
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        PosixShellSandbox::remove_file(self, path).await
    }

    async fn list_files(&self, path: &str) -> Result<Vec<FileInfo>> {
        PosixShellSandbox::list_files(self, path).await
    }

    fn get_tools(&self) -> Vec<Arc<dyn Tool>> {
        PosixShellSandbox::get_tools(self)
    }

    async fn execute(&self, command: &str, options: ExecuteOptions) -> Result<ExecutionResult> {
        PosixShellSandbox::execute(self, command, options).await
    }

    async fn execute_code(
        &self,
        code: &str,
        language: &str,
        options: ExecuteOptions,
    ) -> Result<ExecutionResult> {
        PosixShellSandbox::execute_code(self, code, language, options).await
    }

    async fn read_text(&self, path: &str) -> Result<String> {
        PosixShellSandbox::read_text(self, path).await
    }

    async fn write_text(&self, path: &str, content: &str) -> Result<()> {
        PosixShellSandbox::write_text(self, path, content).await
    }
}

fn command_error(stderr: String, fallback: String) -> Error {
    Error::other(if stderr.is_empty() { fallback } else { stderr })
}

fn parse_listing(stdout: &str) -> Vec<FileInfo> {
    stdout
        .split('\n')
        .map(|line| line.strip_suffix('\r').unwrap_or(line))
        .filter(|line| !line.is_empty() && *line != "./" && *line != "../")
        .filter_map(|line| {
            let (name, is_dir) = match line.strip_suffix('/') {
                Some(name) => (name, true),
                None => (line, false),
            };
            (!name.is_empty()).then(|| FileInfo::new(name).with_is_dir(is_dir))
        })
        .collect()
}
