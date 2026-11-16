//! Ported from harness-ts/test/sandbox.test.ts

use std::collections::VecDeque;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use futures::StreamExt;
use strands::models::model::BoxStream;
use strands::sandbox::{ExecuteOptions, ExecutionEvent, ExecutionResult, FileInfo, Sandbox};

use super::{SHELL_PROBE, SandboxShell, ShellCache};

/// Answers the shell probe from a queue of `(exit code, stdout)`, repeating the last answer, and
/// counts the probes.
struct ProbedSandbox {
    queue: Mutex<VecDeque<(i32, &'static str)>>,
    last: (i32, &'static str),
    probes: AtomicUsize,
}

fn sandbox_answering(results: &[(i32, &'static str)]) -> Arc<ProbedSandbox> {
    Arc::new(ProbedSandbox {
        queue: Mutex::new(results.iter().copied().collect()),
        last: *results.last().unwrap(),
        probes: AtomicUsize::new(0),
    })
}

#[async_trait]
impl Sandbox for ProbedSandbox {
    fn execute_streaming<'a>(
        &'a self,
        _command: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        futures::stream::empty().boxed()
    }

    fn execute_code_streaming<'a>(
        &'a self,
        _code: &'a str,
        _language: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, strands::Result<ExecutionEvent>> {
        futures::stream::empty().boxed()
    }

    async fn execute(&self, command: &str, _options: ExecuteOptions) -> strands::Result<ExecutionResult> {
        if command != SHELL_PROBE {
            return Err(strands::Error::other(format!("unexpected command {command}")));
        }
        self.probes.fetch_add(1, Ordering::SeqCst);
        let (exit_code, stdout) = self.queue.lock().unwrap().pop_front().unwrap_or(self.last);
        Ok(ExecutionResult::new(exit_code, stdout, ""))
    }

    async fn read_file(&self, _path: &str) -> strands::Result<Vec<u8>> {
        Ok(Vec::new())
    }

    async fn write_file(&self, _path: &str, _content: &[u8]) -> strands::Result<()> {
        Ok(())
    }

    async fn remove_file(&self, _path: &str) -> strands::Result<()> {
        Ok(())
    }

    async fn list_files(&self, _path: &str) -> strands::Result<Vec<FileInfo>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn recognises_powershell_by_its_edition_and_caches_the_answer_per_sandbox() {
    let probed = sandbox_answering(&[(0, "Core\r\n")]);
    let sandbox: Arc<dyn Sandbox> = probed.clone();
    let cache = ShellCache::default();
    assert_eq!(cache.shell_of(&sandbox).await.unwrap(), SandboxShell::PowerShell);
    assert_eq!(cache.shell_of(&sandbox).await.unwrap(), SandboxShell::PowerShell);
    assert_eq!(probed.probes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn treats_an_empty_expansion_as_sh() {
    let sandbox: Arc<dyn Sandbox> = sandbox_answering(&[(0, "\n")]);
    assert_eq!(
        ShellCache::default().shell_of(&sandbox).await.unwrap(),
        SandboxShell::Sh
    );
}

#[tokio::test]
async fn does_not_cache_a_failed_probe() {
    let probed = sandbox_answering(&[(1, ""), (0, "Desktop\n")]);
    let sandbox: Arc<dyn Sandbox> = probed.clone();
    let cache = ShellCache::default();
    assert_eq!(cache.shell_of(&sandbox).await.unwrap(), SandboxShell::Sh);
    assert_eq!(cache.shell_of(&sandbox).await.unwrap(), SandboxShell::PowerShell);
    assert_eq!(probed.probes.load(Ordering::SeqCst), 2);
}
