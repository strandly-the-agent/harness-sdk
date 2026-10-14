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
    is_valid_env_key, ExecuteOptions, ExecutionResult, FileInfo, Sandbox, SandboxError, SandboxResult,
};

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

    async fn execute(&self, command: &str, options: ExecuteOptions) -> SandboxResult<ExecutionResult> {
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
        let stdin_pipe = options.stdin.map(|stdin| (child.stdin.take(), stdin));
        let stdout = collect(child.stdout.take());
        let stderr = collect(child.stderr.take());

        // Raced below, never awaited to completion on its own: a child that fills its stdout pipe
        // while we are still writing stdin would otherwise deadlock with no timeout armed.
        let feed_stdin = async {
            if let Some((Some(mut pipe), stdin)) = stdin_pipe {
                let _ = pipe.write_all(stdin.as_bytes()).await;
                let _ = pipe.shutdown().await;
            }
            std::future::pending::<()>().await
        };

        let cancelled = async {
            match &options.cancel {
                Some(token) => token.cancelled().await,
                None => std::future::pending().await,
            }
        };
        let waited = async {
            match options.timeout {
                Some(timeout) => tokio::time::timeout(timeout, child.wait()).await.map_err(|_| ()),
                None => Ok(child.wait().await),
            }
        };

        tokio::select! {
            biased;
            () = feed_stdin => unreachable!("stdin feed never completes"),
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
        self.execute(&format!("{language} -c {}", super::shell_quote(code)), options)
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("strands-sandbox-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    #[tokio::test]
    async fn runs_a_command_on_the_host() {
        let result = LocalSandbox::new()
            .execute("echo hello", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "hello");
        assert_eq!(result.exit_code, 0);
        assert!(result.output_files.is_empty());
    }

    #[tokio::test]
    async fn respects_the_cwd_option() {
        let dir = temp_dir("cwd");
        let result = LocalSandbox::new()
            .execute(
                "pwd",
                ExecuteOptions {
                    cwd: Some(dir.clone()),
                    ..ExecuteOptions::default()
                },
            )
            .await
            .unwrap();
        assert!(result.stdout.contains(dir.file_name().unwrap().to_str().unwrap()));
    }

    #[tokio::test]
    async fn reports_a_non_zero_exit_code_and_stderr_from_a_failing_command() {
        let result = LocalSandbox::new()
            .execute("echo oops >&2; exit 3", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 3);
        assert_eq!(result.stderr.trim(), "oops");
    }

    #[tokio::test]
    async fn applies_the_env_option() {
        let result = LocalSandbox::new()
            .execute(
                "echo \"$MY_VAR\"",
                ExecuteOptions {
                    env: Some(HashMap::from([("MY_VAR".to_owned(), "set".to_owned())])),
                    ..ExecuteOptions::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "set");
    }

    #[tokio::test]
    async fn rejects_invalid_env_var_names() {
        let error = LocalSandbox::new()
            .execute(
                "true",
                ExecuteOptions {
                    env: Some(HashMap::from([("1BAD".to_owned(), "x".to_owned())])),
                    ..ExecuteOptions::default()
                },
            )
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Invalid environment variable name: 1BAD");
    }

    #[tokio::test]
    async fn kills_process_on_timeout_and_reports_the_output_captured_before_the_kill() {
        let error = LocalSandbox::new()
            .execute(
                "echo partial; sleep 10",
                ExecuteOptions::with_timeout(Duration::from_millis(500)),
            )
            .await
            .unwrap_err();
        match error {
            SandboxError::Timeout { stdout, seconds, .. } => {
                assert_eq!(stdout.trim(), "partial");
                assert!((seconds - 0.5).abs() < 1e-9);
            }
            other => panic!("expected a timeout, got {other}"),
        }
    }

    #[tokio::test]
    async fn does_not_timeout_fast_commands() {
        let result = LocalSandbox::new()
            .execute("echo fast", ExecuteOptions::with_timeout(Duration::from_secs(5)))
            .await
            .unwrap();
        assert_eq!(result.stdout.trim(), "fast");
    }

    #[tokio::test]
    async fn rejects_immediately_if_signal_is_already_aborted() {
        let cancel = tokio_util::sync::CancellationToken::new();
        cancel.cancel();
        let error = LocalSandbox::new()
            .execute(
                "sleep 10",
                ExecuteOptions {
                    cancel: Some(cancel),
                    ..ExecuteOptions::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, SandboxError::Aborted), "{error}");
    }

    #[tokio::test]
    async fn kills_process_when_signal_is_aborted() {
        let cancel = tokio_util::sync::CancellationToken::new();
        let token = cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(150)).await;
            token.cancel();
        });
        let started = std::time::Instant::now();
        let error = LocalSandbox::new()
            .execute(
                "sleep 4 & sleep 10",
                ExecuteOptions {
                    cancel: Some(cancel),
                    ..ExecuteOptions::default()
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(error, SandboxError::Aborted), "{error}");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[tokio::test]
    async fn returns_exit_code_127_when_command_is_not_found() {
        let result = LocalSandbox::new()
            .execute("definitely-not-a-binary-xyz", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 127);
    }

    #[tokio::test]
    async fn maps_signal_termination_to_128_plus_signal_number() {
        let result = LocalSandbox::new()
            .execute("kill -TERM $$", ExecuteOptions::default())
            .await
            .unwrap();
        assert_eq!(result.exit_code, 128 + 15);
    }

    #[tokio::test]
    async fn writes_stdin_to_the_command() {
        let result = LocalSandbox::new()
            .execute(
                "cat",
                ExecuteOptions {
                    stdin: Some("piped".to_owned()),
                    ..ExecuteOptions::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(result.stdout, "piped");
    }

    #[tokio::test]
    async fn runs_code_through_the_named_interpreter() {
        let sandbox = LocalSandbox::new();
        let result = sandbox
            .execute_code("print('hi')", "python3", ExecuteOptions::default())
            .await;
        match result {
            Ok(result) => assert_eq!(result.stdout.trim(), "hi"),
            Err(error) => assert_eq!(error.to_string(), "Language interpreter not found: python3"),
        }
    }

    #[tokio::test]
    async fn rejects_a_language_with_invalid_characters() {
        let error = LocalSandbox::new()
            .execute_code("x", "../python", ExecuteOptions::default())
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "language parameter contains invalid characters: ../python"
        );
    }

    #[tokio::test]
    async fn text_file_roundtrip_via_absolute_path() {
        let path = temp_dir("rt").join("note.txt");
        let sandbox = LocalSandbox::new();
        sandbox.write_text(path.to_str().unwrap(), "héllo").await.unwrap();
        assert_eq!(sandbox.read_text(path.to_str().unwrap()).await.unwrap(), "héllo");
    }

    #[tokio::test]
    async fn binary_roundtrip_preserves_all_byte_values() {
        let path = temp_dir("bin").join("bytes.bin");
        let bytes: Vec<u8> = (0..=255u8).collect();
        let sandbox = LocalSandbox::new();
        sandbox.write_file(path.to_str().unwrap(), &bytes).await.unwrap();
        assert_eq!(sandbox.read_file(path.to_str().unwrap()).await.unwrap(), bytes);
    }

    #[tokio::test]
    async fn creates_missing_parent_directories_on_write() {
        let path = temp_dir("parents").join("a/b/c.txt");
        let sandbox = LocalSandbox::new();
        sandbox.write_text(path.to_str().unwrap(), "x").await.unwrap();
        assert_eq!(sandbox.read_text(path.to_str().unwrap()).await.unwrap(), "x");
    }

    #[tokio::test]
    async fn throws_when_reading_a_nonexistent_file() {
        let error = LocalSandbox::new()
            .read_file("/nope/missing.txt")
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Path not found: /nope/missing.txt");
    }

    #[tokio::test]
    async fn removes_a_file_and_throws_on_a_nonexistent_file() {
        let path = temp_dir("rm").join("gone.txt");
        let sandbox = LocalSandbox::new();
        sandbox.write_text(path.to_str().unwrap(), "x").await.unwrap();
        sandbox.remove_file(path.to_str().unwrap()).await.unwrap();
        let error = sandbox.remove_file(path.to_str().unwrap()).await.unwrap_err();
        assert!(matches!(error, SandboxError::PathNotFound(_)), "{error}");
    }

    #[tokio::test]
    async fn returns_entries_sorted_by_name_with_is_dir_and_size_metadata() {
        let dir = temp_dir("list");
        let sandbox = LocalSandbox::with_base_dir(&dir);
        sandbox.write_text("b.txt", "12345").await.unwrap();
        sandbox.write_text("sub/inner.txt", "x").await.unwrap();
        let files = sandbox.list_files(dir.to_str().unwrap()).await.unwrap();
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, vec!["b.txt", "sub"]);
        assert_eq!(files[0].is_dir, Some(false));
        assert_eq!(files[0].size, Some(5));
        assert_eq!(files[1].is_dir, Some(true));
    }

    #[tokio::test]
    async fn throws_path_not_found_on_a_nonexistent_directory_or_file_component() {
        let sandbox = LocalSandbox::new();
        assert!(matches!(
            sandbox.list_files("/nope/nothing").await.unwrap_err(),
            SandboxError::PathNotFound(_)
        ));
        let path = temp_dir("notdir").join("file.txt");
        sandbox.write_text(path.to_str().unwrap(), "x").await.unwrap();
        let error = sandbox
            .list_files(path.join("child").to_str().unwrap())
            .await
            .unwrap_err();
        assert!(matches!(error, SandboxError::PathNotFound(_)), "{error}");
    }

    #[tokio::test]
    async fn resolves_relative_paths_against_the_base_dir() {
        let dir = temp_dir("rel");
        let sandbox = LocalSandbox::with_base_dir(&dir);
        sandbox.write_text("rel.txt", "y").await.unwrap();
        assert_eq!(std::fs::read_to_string(dir.join("rel.txt")).unwrap(), "y");
        assert_eq!(sandbox.read_text("rel.txt").await.unwrap(), "y");
    }

    #[tokio::test]
    async fn handles_multiple_concurrent_commands() {
        let sandbox = Arc::new(LocalSandbox::new());
        let mut handles = Vec::new();
        for i in 0..5 {
            let sandbox = Arc::clone(&sandbox);
            handles.push(tokio::spawn(async move {
                sandbox
                    .execute(&format!("echo {i}"), ExecuteOptions::default())
                    .await
                    .unwrap()
                    .stdout
                    .trim()
                    .to_owned()
            }));
        }
        let mut outputs = Vec::new();
        for handle in handles {
            outputs.push(handle.await.unwrap());
        }
        outputs.sort();
        assert_eq!(outputs, vec!["0", "1", "2", "3", "4"]);
    }

    #[tokio::test]
    async fn yields_stream_chunks_then_execution_result() {
        use futures::StreamExt;
        let sandbox = LocalSandbox::new();
        let events: Vec<_> = sandbox
            .execute_streaming("echo streamed", ExecuteOptions::default())
            .collect()
            .await;
        assert_eq!(events.len(), 1);
        match events.into_iter().next().unwrap().unwrap() {
            super::super::SandboxEvent::Result(result) => {
                assert_eq!(result.stdout.trim(), "streamed");
            }
            other => panic!("expected a result event, got {other:?}"),
        }
    }

    #[test]
    fn vends_no_tools_by_default() {
        assert!(LocalSandbox::new().tools().is_empty());
        assert_eq!(LocalSandbox::new().kind(), "local");
    }
}
