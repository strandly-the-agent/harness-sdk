//! Host execution with no isolation (TS `sandbox/not-a-sandbox-local-environment.ts`).

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use futures::stream;

use crate::error::{
    Error, Result, SandboxIsADirectoryError, SandboxPathNotFoundError, SandboxPermissionError,
};
use crate::models::model::BoxStream;
use crate::retry::model_retry_strategy::{Sleeper, tokio_sleeper};

use super::base::{ExecuteOptions, Sandbox};
use super::constants::{is_valid_language, shell_quote};
use super::posix_shell::{build_shell_env_prefix, code_command, invalid_language, validate_env_keys};
use super::stream_process::{ProcessLimits, ProcessSpec, stream_process};
use super::types::{ExecutionEvent, FileInfo};

const POWERSHELL: &str = "powershell.exe";
const POWERSHELL_NOT_FOUND: &str =
    "PowerShell not found: powershell.exe is required to run commands on Windows";

/// Runs commands and file operations on the host, with no isolation (TS
/// `NotASandboxLocalEnvironment`). The default sandbox of an agent.
///
/// Commands run in `sh -c` on POSIX and in PowerShell on Windows, in the process's working
/// directory unless [`ExecuteOptions::cwd`] is set; relative file paths resolve against the
/// working directory too. Children inherit the process environment.
#[derive(Clone)]
pub struct NotASandboxLocalEnvironment {
    base_dir: Option<PathBuf>,
    sleeper: Sleeper,
    host_env: fn() -> Vec<(String, String)>,
}

impl NotASandboxLocalEnvironment {
    /// The host environment.
    pub fn new() -> Self {
        Self {
            base_dir: None,
            sleeper: tokio_sleeper(),
            host_env,
        }
    }

    /// Resolves relative paths and the default cwd against `dir` instead of the process's
    /// working directory.
    #[cfg_attr(not(test), expect(dead_code, reason = "test seam"))]
    pub(crate) fn with_base_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.base_dir = Some(dir.into());
        self
    }

    /// Waits out timeouts with `sleeper`.
    #[cfg_attr(not(test), expect(dead_code, reason = "test seam"))]
    pub(crate) fn with_sleeper(mut self, sleeper: Sleeper) -> Self {
        self.sleeper = sleeper;
        self
    }

    fn base_dir(&self) -> Result<PathBuf> {
        match &self.base_dir {
            Some(dir) => Ok(dir.clone()),
            None => std::env::current_dir().map_err(Error::other),
        }
    }

    fn resolve_path(&self, path: &str) -> Result<PathBuf> {
        let path = Path::new(path);
        if path.is_absolute() {
            return Ok(path.to_path_buf());
        }
        Ok(self.base_dir()?.join(path))
    }

    fn cwd(&self, options: &ExecuteOptions) -> Result<String> {
        match &options.cwd {
            Some(cwd) => Ok(cwd.clone()),
            None => Ok(self.base_dir()?.to_string_lossy().into_owned()),
        }
    }

    fn limits(&self, options: &ExecuteOptions) -> ProcessLimits {
        ProcessLimits {
            timeout: options.timeout,
            cancel_signal: options.cancel_signal.clone(),
            sleeper: self.sleeper.clone(),
        }
    }

    fn run(
        &self,
        spec: Result<ProcessSpec>,
        options: &ExecuteOptions,
    ) -> BoxStream<'static, Result<ExecutionEvent>> {
        match spec {
            Ok(spec) => stream_process(spec, self.limits(options)),
            Err(error) => Box::pin(stream::once(async move { Err(error) })),
        }
    }
}

impl Default for NotASandboxLocalEnvironment {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for NotASandboxLocalEnvironment {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotASandboxLocalEnvironment")
            .field("base_dir", &self.base_dir)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl Sandbox for NotASandboxLocalEnvironment {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        let spec = self
            .cwd(&options)
            .and_then(|cwd| command_spec(cfg!(windows), command, &cwd, options.env.as_ref(), self.host_env));
        self.run(spec, &options)
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        let spec = self.cwd(&options).and_then(|cwd| {
            code_spec(
                cfg!(windows),
                code,
                language,
                &cwd,
                options.env.as_ref(),
                self.host_env,
            )
        });
        self.run(spec, &options)
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        let full = self.resolve_path(path)?;
        tokio::fs::read(full)
            .await
            .map_err(|error| read_error(path, error))
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        let full = self.resolve_path(path)?;
        if let Some(parent) = full.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(Error::other)?;
        }
        tokio::fs::write(full, content).await.map_err(Error::other)
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        let full = self.resolve_path(path)?;
        tokio::fs::remove_file(full).await.map_err(Error::other)
    }

    async fn list_files(&self, path: &str) -> Result<Vec<FileInfo>> {
        let full = self.resolve_path(path)?;
        let mut entries = tokio::fs::read_dir(&full).await.map_err(|error| {
            if is_missing_path(&error) {
                Error::from(SandboxPathNotFoundError::new(path).with_source(error))
            } else {
                Error::other(error)
            }
        })?;
        let mut names = Vec::new();
        while let Some(entry) = entries.next_entry().await.map_err(Error::other)? {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
        names.sort_by(|a, b| a.to_lowercase().cmp(&b.to_lowercase()).then_with(|| a.cmp(b)));
        let mut files = Vec::with_capacity(names.len());
        for name in names {
            let info = match tokio::fs::metadata(full.join(&name)).await {
                Ok(metadata) => FileInfo::new(name)
                    .with_is_dir(metadata.is_dir())
                    .with_size(metadata.len()),
                Err(_) => FileInfo::new(name),
            };
            files.push(info);
        }
        Ok(files)
    }
}

fn host_env() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .collect()
}

/// A missing entry, or a path component that is not a directory.
fn is_missing_path(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
    )
}

fn read_error(path: &str, error: io::Error) -> Error {
    if is_missing_path(&error) {
        return SandboxPathNotFoundError::new(path).with_source(error).into();
    }
    match error.kind() {
        io::ErrorKind::IsADirectory => SandboxIsADirectoryError::new(path).with_source(error).into(),
        io::ErrorKind::PermissionDenied => SandboxPermissionError::new(path).with_source(error).into(),
        _ => Error::other(error),
    }
}

/// The process that runs `command`: `sh -c 'cd <cwd> && <env prefix><command>'` on POSIX, or
/// PowerShell with cwd and env set on the process on Windows.
pub(crate) fn command_spec(
    windows: bool,
    command: &str,
    cwd: &str,
    env: Option<&BTreeMap<String, String>>,
    host_env: fn() -> Vec<(String, String)>,
) -> Result<ProcessSpec> {
    if windows {
        return Ok(ProcessSpec {
            program: POWERSHELL.to_owned(),
            args: powershell_args(command),
            cwd: Some(PathBuf::from(cwd)),
            env: Some(windows_env(env, host_env)?),
            enoent_message: Some(POWERSHELL_NOT_FOUND.to_owned()),
            ..ProcessSpec::default()
        });
    }
    let prefix = build_shell_env_prefix(env)?;
    Ok(ProcessSpec {
        program: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            format!("cd {} && {prefix}{command}", shell_quote(cwd)),
        ],
        ..ProcessSpec::default()
    })
}

/// The process that runs `code` through `language`: the base64 here-document pipeline in `sh`
/// on POSIX, or the interpreter reading the code from stdin on Windows.
pub(crate) fn code_spec(
    windows: bool,
    code: &str,
    language: &str,
    cwd: &str,
    env: Option<&BTreeMap<String, String>>,
    host_env: fn() -> Vec<(String, String)>,
) -> Result<ProcessSpec> {
    if !is_valid_language(language) {
        return Err(invalid_language(language));
    }
    let enoent_message = Some(format!("Language interpreter not found: {language}"));
    if windows {
        return Ok(ProcessSpec {
            program: language.to_owned(),
            cwd: Some(PathBuf::from(cwd)),
            env: Some(windows_env(env, host_env)?),
            input: Some(code.to_owned()),
            enoent_message,
            ..ProcessSpec::default()
        });
    }
    let prefix = build_shell_env_prefix(env)?;
    let command = code_command(code, language);
    Ok(ProcessSpec {
        program: "sh".to_owned(),
        args: vec![
            "-c".to_owned(),
            format!("cd {} && {prefix}{command}", shell_quote(cwd)),
        ],
        enoent_message,
        ..ProcessSpec::default()
    })
}

fn windows_env(
    env: Option<&BTreeMap<String, String>>,
    host_env: fn() -> Vec<(String, String)>,
) -> Result<Vec<(String, String)>> {
    if let Some(env) = env {
        validate_env_keys(env)?;
    }
    Ok(host_env_with(host_env(), env))
}

/// The host environment with `extra` applied; Windows names are case-insensitive, so `PATH`
/// replaces `Path` (TS `hostEnvWith`).
pub(crate) fn host_env_with(
    mut host: Vec<(String, String)>,
    extra: Option<&BTreeMap<String, String>>,
) -> Vec<(String, String)> {
    for (key, value) in extra.into_iter().flatten() {
        host.retain(|(existing, _)| !existing.eq_ignore_ascii_case(key));
        host.push((key.clone(), value.clone()));
    }
    host
}

/// The PowerShell argv that runs `command` (TS `powershellArgs`).
///
/// `-EncodedCommand` (base64 of UTF-16LE) carries the script without command-line quoting. The
/// wrapper makes output UTF-8 and mirrors `sh -c`'s exit code: the last statement's success, or
/// the failing native command's own code.
pub(crate) fn powershell_args(command: &str) -> Vec<String> {
    let script = [
        "[Console]::OutputEncoding = [System.Text.Encoding]::UTF8",
        "$OutputEncoding = [System.Text.UTF8Encoding]::new($false)",
        command,
        "$ok = $?",
        "if (-not $ok) { if ($LASTEXITCODE) { exit $LASTEXITCODE } else { exit 1 } }",
    ]
    .join("\n");
    let utf16: Vec<u8> = script.encode_utf16().flat_map(u16::to_le_bytes).collect();
    vec![
        "-NoProfile".to_owned(),
        "-NonInteractive".to_owned(),
        "-EncodedCommand".to_owned(),
        STANDARD.encode(utf16),
    ]
}
