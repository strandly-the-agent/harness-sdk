//! Spawn a process and stream its output (TS `sandbox/stream-process.ts`).
//!
//! The child runs in its own process group (a job object on Windows), so a timeout, a cancel or
//! a dropped stream ends the whole tree, not only the shell.

use std::io;
use std::path::PathBuf;
use std::pin::pin;
use std::process::{ExitStatus, Stdio};
use std::time::Duration;

use futures::future::{self, BoxFuture, FutureExt};
use process_wrap::tokio::{ChildWrapper, CommandWrap, KillOnDrop};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tracing::Instrument;

use crate::cancel::CancelSignal;
use crate::error::{Error, Result, SandboxAbortError, SandboxTimeoutError};
use crate::models::model::BoxStream;
use crate::retry::model_retry_strategy::Sleeper;

use super::types::{ExecutionEvent, ExecutionResult, StreamChunk, StreamType};

/// How long a terminated process gets between `SIGTERM` and `SIGKILL` (TS `killTimer`).
const KILL_GRACE: Duration = Duration::from_secs(1);
/// Exit code reported for a missing binary when an `enoent_message` is set.
const NOT_FOUND_EXIT_CODE: i32 = 127;
#[cfg(unix)]
const SIGTERM: i32 = 15;
/// Signals TS maps to `128 + n` (TS `SIGNAL_CODES`); any other signal maps to `128 + 1`.
#[cfg(unix)]
const SIGNAL_CODES: [i32; 8] = [1, 2, 3, 6, 9, 11, 13, 15];

/// What to spawn (TS `streamProcess` arguments and `StreamProcessOptions`).
#[derive(Debug, Default)]
pub(crate) struct ProcessSpec {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
    pub(crate) cwd: Option<PathBuf>,
    /// Replaces the child's environment when set.
    pub(crate) env: Option<Vec<(String, String)>>,
    /// Written to stdin, which is then closed; without it stdin stays an open pipe.
    pub(crate) input: Option<String>,
    /// Reports a missing binary as exit 127 with this text on stderr instead of an error.
    pub(crate) enoent_message: Option<String>,
}

/// When to stop the process early.
pub(crate) struct ProcessLimits {
    pub(crate) timeout: Option<Duration>,
    pub(crate) cancel_signal: Option<CancelSignal>,
    pub(crate) sleeper: Sleeper,
}

/// Spawns `spec` and yields its output chunks, then one result.
///
/// A timeout yields [`SandboxTimeoutError`] with the output so far and a cancel yields
/// [`SandboxAbortError`]; both terminate the process group (`SIGTERM`, then `SIGKILL` after a
/// second). Dropping the stream kills the group.
pub(crate) fn stream_process(
    spec: ProcessSpec,
    limits: ProcessLimits,
) -> BoxStream<'static, Result<ExecutionEvent>> {
    Box::pin(async_stream::stream! {
        if limits.cancel_signal.as_ref().is_some_and(CancelSignal::is_cancelled) {
            yield Err(SandboxAbortError::new().into());
            return;
        }
        let mut child = match spawn(&spec) {
            Ok(child) => ChildGuard::new(child),
            Err(error) => {
                match (error.kind(), &spec.enoent_message) {
                    (io::ErrorKind::NotFound, Some(message)) => {
                        let result = ExecutionResult::new(NOT_FOUND_EXIT_CODE, "", message.clone());
                        yield Ok(ExecutionEvent::ExecutionResult(result));
                    }
                    _ => yield Err(Error::other(error)),
                }
                return;
            }
        };
        let mut stdout_pipe = child.take_stdout();
        let mut stderr_pipe = child.take_stderr();
        let _open_stdin = child.feed_stdin(spec.input);

        let mut deadline = pin!(deadline(&limits));
        let mut cancelled = pin!(cancelled(limits.cancel_signal.clone()));
        let (mut stdout, mut stderr) = (String::new(), String::new());
        let (mut stdout_decoder, mut stderr_decoder) = (Utf8Decoder::default(), Utf8Decoder::default());
        let (mut stdout_buf, mut stderr_buf) = ([0u8; 8192], [0u8; 8192]);

        while stdout_pipe.is_some() || stderr_pipe.is_some() {
            let step = tokio::select! {
                read = read_some(&mut stdout_pipe, &mut stdout_buf) => Step::Read(StreamType::Stdout, read),
                read = read_some(&mut stderr_pipe, &mut stderr_buf) => Step::Read(StreamType::Stderr, read),
                () = &mut deadline => Step::TimedOut,
                () = &mut cancelled => Step::Cancelled,
            };
            let (stream_type, read) = match step {
                Step::Read(stream_type, read) => (stream_type, read),
                Step::TimedOut => {
                    child.terminate(&limits.sleeper);
                    yield Err(timeout_error(&limits, stdout, stderr));
                    return;
                }
                Step::Cancelled | Step::Exited(_) => {
                    child.terminate(&limits.sleeper);
                    yield Err(SandboxAbortError::new().into());
                    return;
                }
            };
            let (pipe, decoder, total) = match stream_type {
                StreamType::Stdout => (&mut stdout_pipe, &mut stdout_decoder, &mut stdout),
                StreamType::Stderr => (&mut stderr_pipe, &mut stderr_decoder, &mut stderr),
            };
            let text = match read {
                Ok(0) | Err(_) => {
                    *pipe = None;
                    decoder.finish()
                }
                Ok(read) => decoder.decode(filled(stream_type, &stdout_buf, &stderr_buf, read)),
            };
            if !text.is_empty() {
                total.push_str(&text);
                yield Ok(ExecutionEvent::StreamChunk(StreamChunk::new(text, stream_type)));
            }
        }

        let step = tokio::select! {
            status = child.wait() => Step::Exited(status),
            () = &mut deadline => Step::TimedOut,
            () = &mut cancelled => Step::Cancelled,
        };
        match step {
            Step::Exited(Ok(status)) => {
                let result = ExecutionResult::new(exit_code(status), stdout, stderr);
                yield Ok(ExecutionEvent::ExecutionResult(result));
            }
            Step::Exited(Err(error)) => yield Err(Error::other(error)),
            Step::TimedOut => {
                child.terminate(&limits.sleeper);
                yield Err(timeout_error(&limits, stdout, stderr));
            }
            Step::Cancelled | Step::Read(..) => {
                child.terminate(&limits.sleeper);
                yield Err(SandboxAbortError::new().into());
            }
        }
    })
}

type Pipe = Box<dyn AsyncRead + Send + Unpin>;

enum Step {
    Read(StreamType, io::Result<usize>),
    Exited(io::Result<ExitStatus>),
    TimedOut,
    Cancelled,
}

fn filled<'a>(stream_type: StreamType, stdout: &'a [u8], stderr: &'a [u8], read: usize) -> &'a [u8] {
    match stream_type {
        StreamType::Stdout => stdout.get(..read).unwrap_or_default(),
        StreamType::Stderr => stderr.get(..read).unwrap_or_default(),
    }
}

fn timeout_error(limits: &ProcessLimits, stdout: String, stderr: String) -> Error {
    let timeout = limits.timeout.unwrap_or_default();
    SandboxTimeoutError::new(timeout)
        .with_output(stdout, stderr)
        .into()
}

fn deadline(limits: &ProcessLimits) -> BoxFuture<'static, ()> {
    match limits.timeout {
        Some(timeout) => (limits.sleeper)(timeout),
        None => future::pending().boxed(),
    }
}

async fn cancelled(signal: Option<CancelSignal>) {
    match signal {
        Some(signal) => signal.cancelled().await,
        None => future::pending().await,
    }
}

/// Reads from `pipe`; never completes once the pipe is closed.
async fn read_some<R: AsyncRead + Unpin>(pipe: &mut Option<R>, buf: &mut [u8]) -> io::Result<usize> {
    match pipe {
        Some(pipe) => pipe.read(buf).await,
        None => future::pending().await,
    }
}

fn spawn(spec: &ProcessSpec) -> io::Result<Box<dyn ChildWrapper>> {
    let mut command = CommandWrap::with_new(&spec.program, |command| {
        command
            .args(&spec.args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(cwd) = &spec.cwd {
            command.current_dir(cwd);
        }
        if let Some(env) = &spec.env {
            command
                .env_clear()
                .envs(env.iter().map(|(key, value)| (key, value)));
        }
    });
    #[cfg(unix)]
    command.wrap(process_wrap::tokio::ProcessGroup::leader());
    #[cfg(windows)]
    command.wrap(process_wrap::tokio::JobObject);
    command.wrap(KillOnDrop);
    command.spawn()
}

/// Owns the child; kills its group on drop unless it exited or was handed to the reaper.
struct ChildGuard {
    child: Option<Box<dyn ChildWrapper>>,
}

impl ChildGuard {
    fn new(child: Box<dyn ChildWrapper>) -> Self {
        Self { child: Some(child) }
    }

    fn take_stdout(&mut self) -> Option<Pipe> {
        let stdout = self.child.as_mut().and_then(|child| child.stdout().take())?;
        Some(Box::new(stdout))
    }

    fn take_stderr(&mut self) -> Option<Pipe> {
        let stderr = self.child.as_mut().and_then(|child| child.stderr().take())?;
        Some(Box::new(stderr))
    }

    /// Writes `input` to stdin and closes it in the background, or returns the open stdin so
    /// it stays open while the stream lives.
    fn feed_stdin(&mut self, input: Option<String>) -> Option<tokio::process::ChildStdin> {
        let stdin = self.child.as_mut().and_then(|child| child.stdin().take())?;
        let Some(input) = input else {
            return Some(stdin);
        };
        let mut stdin = stdin;
        // A child that exits without reading its input makes this write fail; that's not an error.
        tokio::spawn(
            async move {
                let _ = stdin.write_all(input.as_bytes()).await;
                let _ = stdin.shutdown().await;
            }
            .in_current_span(),
        );
        None
    }

    async fn wait(&mut self) -> io::Result<ExitStatus> {
        let Some(child) = self.child.as_mut() else {
            return Err(io::Error::other("the process was already reaped"));
        };
        let status = child.wait().await;
        if status.is_ok() {
            self.child = None;
        }
        status
    }

    /// Asks the group to stop, then kills it after [`KILL_GRACE`] in the background.
    fn terminate(&mut self, sleeper: &Sleeper) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        #[cfg(unix)]
        let _ = child.signal(SIGTERM);
        #[cfg(not(unix))]
        let _ = child.start_kill();
        let grace = sleeper(KILL_GRACE);
        tokio::spawn(
            async move {
                let exited = tokio::select! {
                    status = child.wait() => status.is_ok(),
                    () = grace => false,
                };
                if !exited {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
            .in_current_span(),
        );
    }
}

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

fn exit_code(status: ExitStatus) -> i32 {
    if let Some(code) = status.code() {
        return code;
    }
    #[cfg(unix)]
    if let Some(signal) = std::os::unix::process::ExitStatusExt::signal(&status) {
        return 128 + if SIGNAL_CODES.contains(&signal) { signal } else { 1 };
    }
    1
}

/// Decodes UTF-8 across read boundaries; a sequence split between reads is held until it
/// completes, and invalid bytes become U+FFFD (Node `StringDecoder`).
#[derive(Debug, Default)]
pub(crate) struct Utf8Decoder {
    pending: Vec<u8>,
}

impl Utf8Decoder {
    pub(crate) fn decode(&mut self, bytes: &[u8]) -> String {
        self.pending.extend_from_slice(bytes);
        let mut text = String::new();
        let mut rest = self.pending.as_slice();
        loop {
            match std::str::from_utf8(rest) {
                Ok(valid) => {
                    text.push_str(valid);
                    rest = &[];
                    break;
                }
                Err(error) => {
                    let (valid, after) = rest.split_at(error.valid_up_to());
                    text.push_str(&String::from_utf8_lossy(valid));
                    match error.error_len() {
                        Some(len) => {
                            text.push(char::REPLACEMENT_CHARACTER);
                            rest = after.get(len..).unwrap_or_default();
                        }
                        None => {
                            rest = after;
                            break;
                        }
                    }
                }
            }
        }
        self.pending = rest.to_vec();
        text
    }

    pub(crate) fn finish(&mut self) -> String {
        let text = String::from_utf8_lossy(&self.pending).into_owned();
        self.pending.clear();
        text
    }
}
