//! The sandbox in the agent loop: the default slot, sandbox-vended tool registration, and the
//! `shell` tool routed through a sandbox.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use strands_agents::sandbox::{
    clear_default_sandbox, set_default_sandbox, ExecuteOptions, ExecutionResult, FileInfo, LocalSandbox,
    Sandbox, SandboxResult,
};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::{Tool, ToolContext, ToolRef};
use strands_agents::types::{
    ContentBlock, Role, StopReason, ToolResultBlock, ToolResultContent, ToolResultStatus, ToolSpec,
    ToolUseBlock,
};
use strands_agents::{Agent, Error, Message, ShellTool};

/// A sandbox that records the commands it was asked to run and vends its own `shell` tool.
#[derive(Debug, Default)]
struct RecordingSandbox {
    commands: std::sync::Mutex<Vec<String>>,
}

#[async_trait]
impl Sandbox for RecordingSandbox {
    fn kind(&self) -> &str {
        "recording"
    }

    async fn execute(&self, command: &str, _options: ExecuteOptions) -> SandboxResult<ExecutionResult> {
        self.commands
            .lock()
            .expect("commands poisoned")
            .push(command.to_owned());
        Ok(ExecutionResult {
            stdout: format!("ran: {command}"),
            ..ExecutionResult::default()
        })
    }

    async fn read_file(&self, _path: &str) -> SandboxResult<Vec<u8>> {
        Ok(Vec::new())
    }

    async fn write_file(&self, _path: &str, _content: &[u8]) -> SandboxResult<()> {
        Ok(())
    }

    async fn remove_file(&self, _path: &str) -> SandboxResult<()> {
        Ok(())
    }

    async fn list_files(&self, _path: &str) -> SandboxResult<Vec<FileInfo>> {
        Ok(Vec::new())
    }

    fn tools(&self) -> Vec<ToolRef> {
        vec![Arc::new(ShellTool::new())]
    }
}

/// A user tool that squats on the `shell` name.
#[derive(Debug)]
struct UserShellTool(ToolSpec);

impl UserShellTool {
    fn new() -> Self {
        Self(ToolSpec {
            name: "shell".to_owned(),
            description: "user shell".to_owned(),
            input_schema: None,
            output_schema: None,
            annotations: None,
        })
    }
}

#[async_trait]
impl Tool for UserShellTool {
    fn name(&self) -> &str {
        &self.0.name
    }

    fn description(&self) -> &str {
        &self.0.description
    }

    fn spec(&self) -> &ToolSpec {
        &self.0
    }

    async fn invoke(&self, ctx: ToolContext) -> strands_agents::Result<ToolResultBlock> {
        Ok(ToolResultBlock::success(
            &ctx.tool_use.tool_use_id,
            vec![ToolResultContent::Text("user".to_owned())],
        ))
    }
}

async fn agent_with(builder: strands_agents::AgentBuilder) -> Agent {
    let agent = builder
        .model(MockMessageModel::shared(vec![MockResponse::message(
            Message::assistant("done"),
        )]))
        .build()
        .unwrap();
    agent.initialize().await.unwrap();
    agent
}

#[tokio::test]
async fn returns_the_configured_sandbox_when_one_is_provided() {
    let agent = agent_with(Agent::builder().sandbox(RecordingSandbox::default())).await;
    assert_eq!(agent.sandbox().unwrap().kind(), "recording");
}

#[tokio::test]
async fn default_slot_fallback_and_unconfigured_error() {
    // One test owns the process-wide slot so the cases cannot race each other.
    clear_default_sandbox();
    let agent = agent_with(Agent::builder()).await;
    let error = agent.sandbox().unwrap_err();
    assert!(matches!(error, Error::DefaultNotConfigured(_)), "{error}");
    assert_eq!(
        error.to_string(),
        "No Sandbox configured. Pass a `sandbox` to the Agent to use sandbox features in this environment."
    );

    set_default_sandbox(Arc::new(LocalSandbox::new()));
    assert_eq!(agent.sandbox().unwrap().kind(), "local");

    // `no_sandbox()` is treated the same as unconfigured by the getter.
    let opted_out = agent_with(Agent::builder().no_sandbox()).await;
    assert_eq!(opted_out.sandbox().unwrap().kind(), "local");
    clear_default_sandbox();
}

#[tokio::test]
async fn registers_sandbox_vended_tools_at_initialization() {
    let agent = agent_with(Agent::builder().sandbox(RecordingSandbox::default())).await;
    let names: Vec<String> = agent.tools().iter().map(|t| t.name().to_owned()).collect();
    assert!(names.contains(&"shell".to_owned()), "{names:?}");
}

#[tokio::test]
async fn skips_a_sandbox_vended_tool_when_a_user_tool_has_the_same_name() {
    let agent = agent_with(
        Agent::builder()
            .sandbox(RecordingSandbox::default())
            .tool(Arc::new(UserShellTool::new())),
    )
    .await;
    let tool = agent
        .tools()
        .into_iter()
        .find(|t| t.name() == "shell")
        .expect("shell tool");
    assert_eq!(tool.description(), "user shell");
}

#[tokio::test]
async fn an_opted_out_sandbox_registers_no_tools() {
    let agent = agent_with(Agent::builder().no_sandbox()).await;
    assert!(agent.tools().iter().all(|t| t.name() != "shell"));
}

#[tokio::test]
async fn the_shell_tool_runs_through_the_agents_sandbox_in_the_loop() {
    let sandbox = Arc::new(RecordingSandbox::default());
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock::new(
                    "shell",
                    "t1",
                    json!({ "command": "echo hi" }),
                ))],
            ),
            StopReason::ToolUse,
        ),
        MockResponse::message(Message::assistant("ran it")),
    ]);
    let agent = Agent::builder()
        .model(model)
        .sandbox(Arc::clone(&sandbox) as Arc<dyn Sandbox>)
        .build()
        .unwrap();

    let result = agent.invoke("run echo hi").await.unwrap();

    assert_eq!(result.to_string(), "ran it");
    let messages = agent.messages();
    let ContentBlock::ToolResult(tool_result) = &messages[2].content[0] else {
        panic!("expected a tool result block");
    };
    assert_eq!(tool_result.status, ToolResultStatus::Success);
    assert_eq!(
        *sandbox.commands.lock().expect("commands poisoned"),
        vec!["echo hi".to_owned()]
    );
}

#[tokio::test]
async fn the_shell_tool_falls_back_to_the_local_host_without_an_agent_sandbox() {
    let output = ShellTool::new().execute("echo local", None).await.unwrap();
    assert_eq!(output.output.trim(), "local");
    assert_eq!(output.exit_code, 0);
}

/// A large stdin against a child that fills its own stdout pipe must not deadlock: the write is
/// raced with the timeout, so the call returns rather than hanging forever.
#[tokio::test]
async fn large_stdin_against_a_full_stdout_pipe_returns_promptly() {
    let sandbox = LocalSandbox::new();
    let stdin = "x".repeat(1024 * 1024);
    let started = std::time::Instant::now();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        sandbox.execute(
            "cat",
            ExecuteOptions {
                timeout: Some(std::time::Duration::from_secs(2)),
                stdin: Some(stdin.clone()),
                ..ExecuteOptions::default()
            },
        ),
    )
    .await
    .expect("execute must not hang")
    .expect("cat succeeds");
    assert!(
        started.elapsed() < std::time::Duration::from_secs(10),
        "took {:?}",
        started.elapsed()
    );
    assert_eq!(result.exit_code, 0);
    assert_eq!(result.stdout.len(), stdin.len());
}
