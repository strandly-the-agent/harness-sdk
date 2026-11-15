//! Implementing `Sandbox` and `PosixShellSandbox` from outside the crate.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use strands::models::model::BoxStream;
use strands::sandbox::{
    ExecuteOptions, ExecutionEvent, ExecutionResult, FileInfo, PosixShellSandbox, Sandbox, SandboxSource,
    StreamChunk, StreamType,
};
use strands::{Agent, Result, async_trait};

#[derive(Default)]
struct Recording {
    commands: Mutex<Vec<String>>,
}

#[async_trait]
impl Sandbox for Recording {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        self.commands.lock().unwrap().push(command.to_owned());
        Box::pin(futures::stream::iter([
            Ok(ExecutionEvent::StreamChunk(StreamChunk::new(
                "out",
                StreamType::Stdout,
            ))),
            Ok(ExecutionEvent::ExecutionResult(ExecutionResult::new(
                0, "out", "",
            ))),
        ]))
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        _language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        self.execute_streaming(code, options)
    }

    async fn read_file(&self, _path: &str) -> Result<Vec<u8>> {
        Ok(b"bytes".to_vec())
    }

    async fn write_file(&self, _path: &str, _content: &[u8]) -> Result<()> {
        Ok(())
    }

    async fn remove_file(&self, _path: &str) -> Result<()> {
        Ok(())
    }

    async fn list_files(&self, _path: &str) -> Result<Vec<FileInfo>> {
        Ok(vec![FileInfo::new("a").with_is_dir(false).with_size(1)])
    }
}

struct Remote;

impl PosixShellSandbox for Remote {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        let result = ExecutionResult::new(0, command, "");
        Box::pin(futures::stream::once(async move {
            Ok(ExecutionEvent::ExecutionResult(result))
        }))
    }
}

#[tokio::test]
async fn a_custom_sandbox_gets_the_provided_methods() {
    let sandbox = Arc::new(Recording::default());
    let result = sandbox.execute("ls", ExecuteOptions::default()).await.unwrap();
    assert_eq!(result.stdout, "out");
    assert_eq!(sandbox.read_text("x").await.unwrap(), "bytes");
    assert_eq!(*sandbox.commands.lock().unwrap(), ["ls"]);
}

#[tokio::test]
async fn a_posix_shell_sandbox_is_a_sandbox() {
    let sandbox: Arc<dyn Sandbox> = Arc::new(Remote);
    let events: Vec<_> = sandbox
        .execute_code_streaming("print(1)", "python3", ExecuteOptions::default())
        .collect()
        .await;
    let Some(Ok(ExecutionEvent::ExecutionResult(result))) = events.last() else {
        panic!("expected a result");
    };
    assert!(result.stdout.starts_with("base64 -d << 'STRANDS_EOF_"));
    assert!(result.stdout.contains("| python3\ncHJpbnQoMSk=\nSTRANDS_EOF_"));
}

#[test]
fn an_agent_takes_a_custom_sandbox() {
    let sandbox: Arc<dyn Sandbox> = Arc::new(Recording::default());
    let agent = Agent::builder()
        .model(Arc::new(strands::testing::MockMessageModel::new()) as Arc<dyn strands::models::model::Model>)
        .sandbox(SandboxSource::from(Arc::clone(&sandbox)))
        .printer(false)
        .build()
        .unwrap();
    assert!(Arc::ptr_eq(&agent.sandbox().unwrap(), &sandbox));
}
