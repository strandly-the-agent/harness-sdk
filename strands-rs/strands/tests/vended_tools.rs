//! The file tools route through the agent's sandbox in a real agent loop.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use strands::models::model::{BoxStream, Model};
use strands::sandbox::{ExecuteOptions, ExecutionEvent, ExecutionResult, FileInfo, Sandbox, SandboxSource};
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::vended_tools::{edit, read, write};
use strands::{Agent, Result, async_trait, tool_list};

/// An in-memory sandbox that records every file operation.
#[derive(Default)]
struct MemorySandbox {
    files: Mutex<BTreeMap<String, Vec<u8>>>,
    log: Mutex<Vec<String>>,
}

#[async_trait]
impl Sandbox for MemorySandbox {
    fn execute_streaming<'a>(
        &'a self,
        _command: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        let result = ExecutionResult::new(0, "", "");
        Box::pin(futures::stream::once(async move {
            Ok(ExecutionEvent::ExecutionResult(result))
        }))
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        _language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        self.execute_streaming(code, options)
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.log.lock().unwrap().push(format!("read {path}"));
        self.files
            .lock()
            .unwrap()
            .get(path)
            .cloned()
            .ok_or_else(|| strands::error::SandboxPathNotFoundError::new(path).into())
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        self.log.lock().unwrap().push(format!("write {path}"));
        self.files
            .lock()
            .unwrap()
            .insert(path.to_owned(), content.to_vec());
        Ok(())
    }

    async fn remove_file(&self, _path: &str) -> Result<()> {
        Ok(())
    }

    async fn list_files(&self, _path: &str) -> Result<Vec<FileInfo>> {
        Ok(Vec::new())
    }
}

#[tokio::test(flavor = "current_thread")]
async fn write_edit_and_read_go_through_the_agent_sandbox() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![
        ToolUseBlock::new("write", "t1", json!({ "path": "/notes.txt", "content": "hello" })).into(),
    ]);
    model.add_turn(vec![
        ToolUseBlock::new(
            "edit",
            "t2",
            json!({ "path": "/notes.txt", "old_str": "hello", "new_str": "bye" }),
        )
        .into(),
    ]);
    model.add_turn(vec![
        ToolUseBlock::new("read", "t3", json!({ "path": "/notes.txt" })).into(),
    ]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let sandbox = Arc::new(MemorySandbox::default());
    let mut agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn Model>)
        .sandbox(SandboxSource::from(Arc::clone(&sandbox) as Arc<dyn Sandbox>))
        .tools(tool_list![write(), edit(), read()])
        .printer(false)
        .build()
        .unwrap();
    agent.invoke("Go").await.unwrap();
    assert_eq!(
        *sandbox.log.lock().unwrap(),
        [
            "write /notes.txt",
            "read /notes.txt",
            "write /notes.txt",
            "read /notes.txt"
        ]
    );
    assert_eq!(sandbox.files.lock().unwrap()["/notes.txt"], b"bye");
    let last_request = model.requests().pop().unwrap();
    let rendered = serde_json::to_string(&last_request.messages).unwrap();
    assert!(rendered.contains("     1\\tbye"), "{rendered}");
}

#[tokio::test]
async fn file_tools_fail_without_a_sandbox() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![
        ToolUseBlock::new("read", "t1", json!({ "path": "/x.txt" })).into(),
    ]);
    model.add_turn(vec![TextBlock::new("done").into()]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model) as Arc<dyn Model>)
        .sandbox(SandboxSource::Off)
        .tools([read()])
        .printer(false)
        .build()
        .unwrap();
    agent.invoke("Go").await.unwrap();
    let rendered = serde_json::to_string(&model.requests().pop().unwrap().messages).unwrap();
    assert!(rendered.contains("Error: No Sandbox configured."), "{rendered}");
}
