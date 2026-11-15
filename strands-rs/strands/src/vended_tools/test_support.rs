//! Helpers for vended-tool tests (TS `createMockAgent` with an `extra.sandbox`).

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use futures::StreamExt;

use crate::JsonValue;
use crate::error::Result;
use crate::models::model::BoxStream;
use crate::sandbox::{
    AgentSandbox, ExecuteOptions, ExecutionEvent, FileInfo, NotASandboxLocalEnvironment, Sandbox,
    SandboxSource,
};
use crate::testing::MockMessageModel;
use crate::tools::tool::ToolStreamItem;
use crate::tools::{Tool, ToolContext, ToolOutcome};
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::messages::ToolResultBlock;
use crate::types::tools::ToolUse;

/// An agent whose sandbox is `sandbox` (the default local one when `None`).
pub(crate) fn agent_with(sandbox: Option<Arc<dyn Sandbox>>) -> LocalAgent {
    let mut agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    agent.set_sandbox(AgentSandbox::from_source(sandbox.map(SandboxSource::from)));
    agent
}

/// A tool context for `name` over `agent`.
pub(crate) fn context_for(agent: &LocalAgent, name: &str, input: JsonValue) -> ToolContext {
    ToolContext::new(
        ToolUse::new(name, "test-id", input),
        agent.tool_agent(),
        InvocationState::new(),
    )
}

/// A tool context whose agent sandbox is `sandbox`.
pub(crate) fn context(sandbox: Option<Arc<dyn Sandbox>>) -> ToolContext {
    context_for(&agent_with(sandbox), "tool", JsonValue::Null)
}

/// Runs `tool` through its stream and returns the result block.
pub(crate) async fn stream_result(tool: &dyn Tool, context: ToolContext) -> ToolResultBlock {
    let items: Vec<ToolStreamItem> = tool.stream(context).collect().await;
    match items.into_iter().last() {
        Some(ToolStreamItem::Outcome(ToolOutcome::Result(result))) => result,
        other => panic!("expected a result, got {other:?}"),
    }
}

/// The local environment as a shared sandbox.
pub(crate) fn local() -> Arc<dyn Sandbox> {
    Arc::new(NotASandboxLocalEnvironment::new())
}

/// One recorded sandbox call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Call {
    Execute(String, ExecuteOptionsSeen),
    ExecuteCode(String, String, ExecuteOptionsSeen),
    ReadFile(String),
    WriteFile(String, Vec<u8>),
}

/// The parts of [`ExecuteOptions`] a test compares.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ExecuteOptionsSeen {
    pub(crate) timeout: Option<std::time::Duration>,
    pub(crate) cancel_signal: bool,
}

impl From<&ExecuteOptions> for ExecuteOptionsSeen {
    fn from(options: &ExecuteOptions) -> Self {
        Self {
            timeout: options.timeout,
            cancel_signal: options.cancel_signal.is_some(),
        }
    }
}

/// Wraps a sandbox and records every call (section 11 item 36).
pub(crate) struct RecordingSandbox {
    pub(crate) inner: Arc<dyn Sandbox>,
    pub(crate) calls: Mutex<Vec<Call>>,
}

impl RecordingSandbox {
    pub(crate) fn new(inner: Arc<dyn Sandbox>) -> Arc<Self> {
        Arc::new(Self {
            inner,
            calls: Mutex::new(Vec::new()),
        })
    }

    pub(crate) fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    fn record(&self, call: Call) {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call);
    }
}

#[async_trait]
impl Sandbox for RecordingSandbox {
    fn execute_streaming<'a>(
        &'a self,
        command: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        self.record(Call::Execute(command.to_owned(), (&options).into()));
        self.inner.execute_streaming(command, options)
    }

    fn execute_code_streaming<'a>(
        &'a self,
        code: &'a str,
        language: &'a str,
        options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        self.record(Call::ExecuteCode(
            code.to_owned(),
            language.to_owned(),
            (&options).into(),
        ));
        self.inner.execute_code_streaming(code, language, options)
    }

    async fn read_file(&self, path: &str) -> Result<Vec<u8>> {
        self.record(Call::ReadFile(path.to_owned()));
        self.inner.read_file(path).await
    }

    async fn write_file(&self, path: &str, content: &[u8]) -> Result<()> {
        self.record(Call::WriteFile(path.to_owned(), content.to_vec()));
        self.inner.write_file(path, content).await
    }

    async fn remove_file(&self, path: &str) -> Result<()> {
        self.inner.remove_file(path).await
    }

    async fn list_files(&self, path: &str) -> Result<Vec<FileInfo>> {
        self.inner.list_files(path).await
    }
}

/// A sandbox whose every operation fails, to prove a tool used another one.
pub(crate) struct ExplodingSandbox;

fn explode<T>() -> Result<T> {
    Err(crate::Error::other("agent sandbox used despite bound sandbox"))
}

#[async_trait]
impl Sandbox for ExplodingSandbox {
    fn execute_streaming<'a>(
        &'a self,
        _command: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        Box::pin(futures::stream::once(async { explode() }))
    }

    fn execute_code_streaming<'a>(
        &'a self,
        _code: &'a str,
        _language: &'a str,
        _options: ExecuteOptions,
    ) -> BoxStream<'a, Result<ExecutionEvent>> {
        Box::pin(futures::stream::once(async { explode() }))
    }

    async fn read_file(&self, _path: &str) -> Result<Vec<u8>> {
        explode()
    }

    async fn write_file(&self, _path: &str, _content: &[u8]) -> Result<()> {
        explode()
    }

    async fn remove_file(&self, _path: &str) -> Result<()> {
        explode()
    }

    async fn list_files(&self, _path: &str) -> Result<Vec<FileInfo>> {
        explode()
    }
}

/// The text of a text content block.
pub(crate) fn text_of(content: &crate::types::messages::ToolResultContent) -> &str {
    match content {
        crate::types::messages::ToolResultContent::Text(block) => block.text(),
        other => panic!("expected text, got {other:?}"),
    }
}
