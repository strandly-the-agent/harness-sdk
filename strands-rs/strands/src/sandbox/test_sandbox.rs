//! Test doubles for sandbox tests (TS `__fixtures__/test-sandbox.node.ts`).

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use futures::StreamExt;
use futures::future::FutureExt;
use tokio::sync::Notify;

use crate::error::Result;
use crate::models::model::BoxStream;
use crate::retry::model_retry_strategy::Sleeper;

use super::NotASandboxLocalEnvironment;
use super::base::{ExecuteOptions, Sandbox};
use super::constants::shell_quote;
use super::posix_shell::{PosixShellSandbox, build_shell_env_prefix};
use super::stream_process::{ProcessLimits, ProcessSpec, stream_process};
use super::types::{ExecutionEvent, StreamType};

/// A sleeper that returns at once, so every timeout fires immediately.
pub(crate) fn instant_sleeper() -> Sleeper {
    Arc::new(|_| async { tokio::task::yield_now().await }.boxed())
}

/// A sleeper that returns when `notify` fires; records each requested delay.
pub(crate) fn manual_sleeper(notify: Arc<Notify>, delays: Arc<Mutex<Vec<Duration>>>) -> Sleeper {
    Arc::new(move |delay| {
        delays.lock().unwrap_or_else(PoisonError::into_inner).push(delay);
        let notify = Arc::clone(&notify);
        async move { notify.notified().await }.boxed()
    })
}

/// A sleeper that never returns.
pub(crate) fn never_sleeper() -> Sleeper {
    Arc::new(|_| futures::future::pending().boxed())
}

/// A local environment rooted at `dir` whose timeouts wait on `sleeper`.
pub(crate) fn local_env(dir: &Path, sleeper: Sleeper) -> NotASandboxLocalEnvironment {
    NotASandboxLocalEnvironment::new()
        .with_base_dir(dir)
        .with_sleeper(sleeper)
}

/// A `PosixShellSandbox` that runs `sh -c 'cd <dir> && ...'` locally.
pub(crate) struct TestSandbox {
    pub(crate) working_dir: PathBuf,
    pub(crate) sleeper: Sleeper,
}

impl TestSandbox {
    pub(crate) fn new(working_dir: impl Into<PathBuf>) -> Self {
        Self {
            working_dir: working_dir.into(),
            sleeper: never_sleeper(),
        }
    }

    pub(crate) fn with_sleeper(mut self, sleeper: Sleeper) -> Self {
        self.sleeper = sleeper;
        self
    }
}

impl PosixShellSandbox for TestSandbox {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        let cwd = options
            .cwd
            .clone()
            .unwrap_or_else(|| self.working_dir.to_string_lossy().into_owned());
        let prefix = match build_shell_env_prefix(options.env.as_ref()) {
            Ok(prefix) => prefix,
            Err(error) => return Box::pin(futures::stream::once(async move { Err(error) })),
        };
        let spec = ProcessSpec {
            program: "sh".to_owned(),
            args: vec![
                "-c".to_owned(),
                format!("cd {} && {prefix}{command}", shell_quote(&cwd)),
            ],
            ..ProcessSpec::default()
        };
        stream_process(
            spec,
            ProcessLimits {
                timeout: options.timeout,
                cancel_signal: options.cancel_signal,
                sleeper: self.sleeper.clone(),
            },
        )
    }
}

/// Wraps a sandbox and fires `notify` once its output contains every `(stream, text)` pair, so a
/// manual-sleeper timeout fires only after that output was read.
pub(crate) struct NotifyOnOutput<S> {
    pub(crate) inner: S,
    pub(crate) notify: Arc<Notify>,
    pub(crate) expected: Vec<(StreamType, &'static str)>,
}

#[async_trait::async_trait]
impl<S: Sandbox> Sandbox for NotifyOnOutput<S> {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        let inner = self.inner.execute_streaming(command, options);
        self.watch(inner)
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        let inner = self.inner.execute_code_streaming(code, language, options);
        self.watch(inner)
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.inner.read_file(path).await
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        self.inner.write_file(path, content).await
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        self.inner.remove_file(path).await
    }

    async fn list_files(&self, path: &str) -> Result<Vec<super::FileInfo>> {
        self.inner.list_files(path).await
    }
}

impl<S: Sandbox> NotifyOnOutput<S> {
    fn watch<'a>(
        &'a self,
        mut inner: BoxStream<'a, Result<ExecutionEvent>>,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        Box::pin(async_stream::stream! {
            let (mut stdout, mut stderr) = (String::new(), String::new());
            let mut fired = false;
            while let Some(event) = inner.next().await {
                if let Ok(ExecutionEvent::StreamChunk(chunk)) = &event {
                    match chunk.stream_type {
                        StreamType::Stdout => stdout.push_str(&chunk.data),
                        StreamType::Stderr => stderr.push_str(&chunk.data),
                    }
                    let seen = self.expected.iter().all(|(stream, text)| match stream {
                        StreamType::Stdout => stdout.contains(text),
                        StreamType::Stderr => stderr.contains(text),
                    });
                    if seen && !fired {
                        fired = true;
                        self.notify.notify_one();
                    }
                }
                yield event;
            }
        })
    }
}
