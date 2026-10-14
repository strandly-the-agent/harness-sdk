//! Host execution with no isolation: the port of TS `NotASandboxLocalEnvironment`.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::process::Command;

use super::{
    is_valid_env_key, ExecuteOptions, ExecutionResult, FileInfo, Sandbox, SandboxError,
    SandboxResult,
};

/// Exit code reported for a command killed by its timeout (the `timeout(1)` convention).
const TIMEOUT_EXIT_CODE: i32 = 124;
/// How long the pipe collectors get to finish after the command ended, before their buffers are
/// taken as-is. Bounds the call when a grandchild still holds the inherited pipes open.
const DRAIN_GRACE: Duration = Duration::from_millis(250);

/// Runs commands and file operations directly on the host — **no isolation whatsoever**.
///
/// The alarming TS name (`NotASandboxLocalEnvironment`) is a deliberate safety signal; this is
/// the same thing. Commands run through `sh -c` in their own process group, so a timeout or a
/// cancelled call kills the whole tree.
#[derive(Clone, Debug, Default)]
pub struct LocalSandbox {
    base_dir: Option<PathBuf>,
    env: Option<HashMap<String, String>>,
}

impl LocalSandbox {
    /// Local sandbox resolving relative paths against the process working directory.
    pub fn new() -> Self {
        Self::default()
    }

    /// Local sandbox resolving relative paths and running commands in `base_dir`.
    pub fn with_base_dir(base_dir: impl Into<PathBuf>) -> Self {
        Self {
            base_dir: Some(base_dir.into()),
            env: None,
        }
    }

    /// Environment variables added to every command.
    pub fn with_env(mut self, env: HashMap<String, String>) -> Self {
        self.env = Some(env);
        self
    }

    fn resolve(&self, path: &str) -> PathBuf {
        let path = Path::new(path);
        match (&self.base_dir, path.is_absolute()) {
            (Some(base), false) => base.join(path),
            _ => path.to_path_buf(),
        }
    }
}

fn path_error(path: &str, error: &std::io::Error, fallback: String) -> SandboxError {
    match error.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory => {
            SandboxError::PathNotFound(path.to_owned())
        }
        _ => SandboxError::Other(fallback),
    }
}

#[async_trait]
impl Sandbox for LocalSandbox {
    fn kind(&self) -> &str {
        "local"
    }

    fn description(&self) -> &str {
        "the local host environment (no isolation)"
    }

    async fn execute(
        &self,
        command: &str,
        options: ExecuteOptions,
    ) -> SandboxResult<ExecutionResult> {
        for key in options.env.iter().flat_map(|env| env.keys()) {
            if !is_valid_env_key(key) {
                return Err(SandboxError::Other(format!(
                    "Invalid environment variable name: {key}"
                )));
            }
        }
        if let Some(cancel) = &options.cancel {
            if cancel.is_cancelled() {
                return Err(SandboxError::Aborted);
            }
        }

        let mut cmd = Command::new("sh");
        cmd.arg("-c")
            .arg(command)
            .stdin(if options.stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        // Own process group, so the whole tree (not just `sh`) can be signalled.
        #[cfg(unix)]
        cmd.process_group(0);
        if let Some(cwd) = options.cwd.as_ref().or(self.base_dir.as_ref()) {
            cmd.current_dir(cwd);
        }
        if let Some(env) = &self.env {
            cmd.envs(env);
        }
        if let Some(env) = &options.env {
            cmd.envs(env);
        }

        let mut child = cmd.spawn().map_err(|e| SandboxError::Other(e.to_string()))?;
        // Kills the group on every exit path, including when this future is dropped.
        let pid = child.id();
        let _group = GroupKill(pid);
        if let Some(stdin) = options.stdin {
            if let Some(mut pipe) = child.stdin.take() {
                let _ = pipe.write_all(stdin.as_bytes()).await;
                let _ = pipe.shutdown().await;
            }
        }
        let stdout = collect(child.stdout.take());
        let stderr = collect(child.stderr.take());

        let cancelled = async {
            match &options.cancel {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let waited = async {
            match options.timeout {
                Some(timeout) => tokio::time::timeout(timeout, child.wait())
                    .await
                    .map_err(|_| ()),
                None => Ok(child.wait().await),
            }
        };

        tokio::select! {
            biased;
            () = cancelled => {
                kill_group(pid);
                let _ = child.kill().await;
                Err(SandboxError::Aborted)
            }
            status = waited => match status {
                Ok(status) => {
                    let status = status.map_err(|e| SandboxError::Other(e.to_string()))?;
                    kill_group(pid);
                    Ok(ExecutionResult {
                        stdout: stdout.read(DRAIN_GRACE).await,
                        stderr: stderr.read(DRAIN_GRACE).await,
                        exit_code: exit_code_of(&status),
                        output_files: Vec::new(),
                    })
                }
                Err(()) => {
                    kill_group(pid);
                    let _ = child.kill().await;
                    Err(SandboxError::Timeout {
                        seconds: options.timeout.unwrap_or_default().as_secs_f64(),
                        stdout: stdout.read(DRAIN_GRACE).await,
                        stderr: stderr.read(DRAIN_GRACE).await,
                    })
                }
            },
        }
    }

    async fn execute_code(
        &self,
        code: &str,
        language: &str,
        options: ExecuteOptions,
    ) -> SandboxResult<ExecutionResult> {
        if !super::is_valid_language(language) {
            return Err(SandboxError::Other(format!(
                "language parameter contains invalid characters: {language}"
            )));
        }
        let result = self
            .execute(
                &format!("command -v {language} >/dev/null 2>&1 || exit 127"),
                ExecuteOptions::default(),
            )
            .await?;
        if result.exit_code == 127 {
            return Err(SandboxError::Other(format!(
                "Language interpreter not found: {language}"
            )));
        }
        self.execute(
            &format!("{language} -c {}", super::shell_quote(code)),
            options,
        )
        .await
    }

    async fn read_file(&self, path: &str) -> SandboxResult<Vec<u8>> {
        let resolved = self.resolve(path);
        tokio::fs::read(&resolved)
            .await
            .map_err(|e| path_error(path, &e, format!("Failed to read file: {path}")))
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> SandboxResult<()> {
        let resolved = self.resolve(path);
        if let Some(parent) = resolved.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| SandboxError::Other(format!("Failed to write file: {path}: {e}")))?;
        }
        tokio::fs::write(&resolved, content)
            .await
            .map_err(|e| path_error(path, &e, format!("Failed to write file: {path}")))
    }

    async fn remove_file(&self, path: &str) -> SandboxResult<()> {
        let resolved = self.resolve(path);
        tokio::fs::remove_file(&resolved)
            .await
            .map_err(|e| path_error(path, &e, format!("Failed to remove file: {path}")))
    }

    async fn list_files(&self, path: &str) -> SandboxResult<Vec<FileInfo>> {
        let resolved = self.resolve(path);
        let mut entries = tokio::fs::read_dir(&resolved)
            .await
            .map_err(|e| path_error(path, &e, format!("Failed to list directory: {path}")))?;
        let mut files = Vec::new();
        while let Some(entry) = entries
            .next_entry()
            .await
            .map_err(|e| path_error(path, &e, format!("Failed to list directory: {path}")))?
        {
            let metadata = entry.metadata().await.ok();
            files.push(FileInfo {
                name: entry.file_name().to_string_lossy().into_owned(),
                is_dir: metadata.as_ref().map(std::fs::Metadata::is_dir),
                size: metadata.as_ref().map(std::fs::Metadata::len),
            });
        }
        files.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(files)
    }
}

#[cfg(unix)]
fn exit_code_of(status: &std::process::ExitStatus) -> i32 {
    use std::os::unix::process::ExitStatusExt;
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(-1)
}

#[cfg(not(unix))]
fn exit_code_of(status: &std::process::ExitStatus) -> i32 {
    status.code().unwrap_or(-1)
}

struct Collected {
    buffer: Arc<Mutex<Vec<u8>>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl Collected {
    /// Wait up to `grace` for EOF, then take whatever has been buffered so far.
    async fn read(mut self, grace: Duration) -> String {
        if let Some(mut task) = self.task.take() {
            if tokio::time::timeout(grace, &mut task).await.is_err() {
                task.abort();
            }
        }
        let bytes = self.buffer.lock().expect("output buffer poisoned").clone();
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Drop for Collected {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

fn collect<R>(reader: Option<R>) -> Collected
where
    R: AsyncReadExt + Unpin + Send + 'static,
{
    let buffer = Arc::new(Mutex::new(Vec::new()));
    let task = reader.map(|mut reader| {
        let buffer = Arc::clone(&buffer);
        tokio::spawn(async move {
            let mut chunk = [0u8; 8192];
            while let Ok(read) = reader.read(&mut chunk).await {
                if read == 0 {
                    break;
                }
                buffer
                    .lock()
                    .expect("output buffer poisoned")
                    .extend_from_slice(&chunk[..read]);
            }
        })
    });
    Collected { buffer, task }
}

/// SIGKILLs the process group of `pid` when dropped (timeout, error, or a cancelled call).
struct GroupKill(Option<u32>);

impl Drop for GroupKill {
    fn drop(&mut self) {
        kill_group(self.0);
    }
}

/// SIGKILL the whole process group led by `pid`; children of the command die with it.
#[cfg(unix)]
fn kill_group(pid: Option<u32>) {
    let Some(pid) = pid.and_then(|pid| i32::try_from(pid).ok()) else {
        return;
    };
    let _ = nix::sys::signal::killpg(nix::unistd::Pid::from_raw(pid), nix::sys::signal::SIGKILL);
}

#[cfg(not(unix))]
fn kill_group(_pid: Option<u32>) {}

/// Exit code a timed-out command is reported with by the shell tool.
pub(crate) const fn timeout_exit_code() -> i32 {
    TIMEOUT_EXIT_CODE
}
