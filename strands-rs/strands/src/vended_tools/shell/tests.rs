//! Ported from strands-ts/src/vended-tools/shell/__tests__/shell.test.node.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::Notify;

use super::*;
use crate::sandbox::StreamType;
use crate::sandbox::test_sandbox::{NotifyOnOutput, TestSandbox, instant_sleeper, manual_sleeper};
use crate::tools::Tool;
use crate::types::tools::ToolResultStatus;
use crate::vended_tools::test_support::{
    Call, RecordingSandbox, agent_with, context, context_for, stream_result, text_of,
};

fn sandbox_shell(sandbox: TestSandbox) -> (tempfile::TempDir, FunctionTool) {
    let dir = tempfile::tempdir().unwrap();
    let sandbox = TestSandbox {
        working_dir: dir.path().to_path_buf(),
        ..sandbox
    };
    let tool = make_shell(MakeShellOptions {
        sandbox: Some(Arc::new(sandbox)),
        ..MakeShellOptions::default()
    })
    .unwrap();
    (dir, tool)
}

fn shell() -> (tempfile::TempDir, FunctionTool) {
    sandbox_shell(TestSandbox::new("/"))
}

fn output(value: JsonValue) -> ShellOutput {
    serde_json::from_value(value).unwrap()
}

#[tokio::test]
async fn executes_command_via_sandbox() {
    let (_dir, tool) = shell();
    let result = output(
        tool.invoke(json!({ "command": "echo \"hello sandbox\"" }), context(None))
            .await
            .unwrap(),
    );
    assert!(result.output.contains("hello sandbox"));
    assert_eq!(result.error, "");
    assert_eq!(result.exit_code, 0);
}

#[tokio::test]
async fn reports_a_non_zero_exit_code() {
    let (_dir, tool) = shell();
    let result = output(
        tool.invoke(json!({ "command": "exit 42" }), context(None))
            .await
            .unwrap(),
    );
    assert_eq!(result.exit_code, 42);
}

#[tokio::test]
async fn captures_stderr_via_sandbox() {
    let (_dir, tool) = shell();
    let result = output(
        tool.invoke(json!({ "command": "echo \"oops\" >&2" }), context(None))
            .await
            .unwrap(),
    );
    assert!(result.error.contains("oops"));
    assert_eq!(result.exit_code, 0);
}

#[tokio::test]
async fn does_not_persist_state_between_calls_stateless() {
    let (_dir, tool) = shell();
    tool.invoke(json!({ "command": "export MY_VAR=hello" }), context(None))
        .await
        .unwrap();
    let result = output(
        tool.invoke(json!({ "command": "echo \"${MY_VAR:-empty}\"" }), context(None))
            .await
            .unwrap(),
    );
    assert_eq!(result.output.trim(), "empty");
}

#[tokio::test]
async fn respects_timeout() {
    let (_dir, tool) = sandbox_shell(TestSandbox::new("/").with_sleeper(instant_sleeper()));
    let result = tool
        .invoke(json!({ "command": "sleep 10", "timeout": 0.1 }), context(None))
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn throws_shell_timeout_error_on_timeout() {
    let (_dir, tool) = sandbox_shell(TestSandbox::new("/").with_sleeper(instant_sleeper()));
    let error = tool
        .invoke(json!({ "command": "sleep 10", "timeout": 0.1 }), context(None))
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Execution timed out after 0.1 seconds\n{\"output\":\"\",\"error\":\"\",\"exit_code\":124}"
    );
}

fn notifying_shell(expected: Vec<(StreamType, &'static str)>) -> (tempfile::TempDir, FunctionTool) {
    let dir = tempfile::tempdir().unwrap();
    let notify = Arc::new(Notify::new());
    let sleeper = manual_sleeper(Arc::clone(&notify), Arc::new(Mutex::new(Vec::new())));
    let sandbox = NotifyOnOutput {
        inner: TestSandbox::new(dir.path()).with_sleeper(sleeper),
        notify,
        expected,
    };
    let tool = make_shell(MakeShellOptions {
        sandbox: Some(Arc::new(sandbox)),
        ..MakeShellOptions::default()
    })
    .unwrap();
    (dir, tool)
}

#[tokio::test]
async fn timeout_error_carries_the_partial_output_with_the_same_field_names_as_a_success_result() {
    let (_dir, tool) = notifying_shell(vec![
        (StreamType::Stdout, "partial\n"),
        (StreamType::Stderr, "warn\n"),
    ]);
    let error = tool
        .invoke(
            json!({ "command": "echo partial; echo warn >&2; sleep 5", "timeout": 0.3 }),
            context(None),
        )
        .await
        .unwrap_err();
    let partial = json!({ "output": "partial\n", "error": "warn\n", "exit_code": 124 });
    assert_eq!(
        error.to_string(),
        format!("Execution timed out after 0.3 seconds\n{partial}")
    );
}

#[tokio::test]
async fn timeout_surfaces_as_a_failed_tool_result_that_still_includes_the_partial_output() {
    let (_dir, tool) = notifying_shell(vec![(StreamType::Stdout, "partial\n")]);
    let agent = agent_with(None);
    let input = json!({ "command": "echo partial; sleep 5", "timeout": 0.3 });
    let result = stream_result(&tool, context_for(&agent, "shell", input)).await;
    assert_eq!(result.status(), &ToolResultStatus::Error);
    let text = text_of(&result.content()[0]).to_owned();
    let (head, json_line) = text.split_once('\n').unwrap();
    assert_eq!(head, "Error: Execution timed out after 0.3 seconds");
    assert_eq!(
        serde_json::from_str::<JsonValue>(json_line).unwrap(),
        json!({ "output": "partial\n", "error": "", "exit_code": 124 })
    );
}

#[test]
fn shows_the_model_the_default_in_the_schema() {
    for (options, expected) in [
        (MakeShellOptions::default(), 120),
        (
            MakeShellOptions {
                default_timeout: Some(Duration::from_secs(300)),
                ..MakeShellOptions::default()
            },
            300,
        ),
    ] {
        let spec = make_shell(options).unwrap().tool_spec();
        let schema = spec.input_schema().unwrap();
        assert_eq!(schema["required"], json!(["command"]));
        assert_eq!(
            schema["properties"]["timeout"],
            json!({
                "default": expected,
                "description": "Optional per-execution timeout in seconds.",
                "type": "number",
                "exclusiveMinimum": 0,
            })
        );
    }
}

fn recording() -> (tempfile::TempDir, Arc<RecordingSandbox>, FunctionTool) {
    let dir = tempfile::tempdir().unwrap();
    let sandbox = RecordingSandbox::new(Arc::new(TestSandbox::new(dir.path())));
    let tool = make_shell(MakeShellOptions {
        sandbox: Some(Arc::clone(&sandbox) as Arc<dyn crate::sandbox::Sandbox>),
        default_timeout: Some(Duration::from_secs(300)),
        ..MakeShellOptions::default()
    })
    .unwrap();
    (dir, sandbox, tool)
}

fn timeouts(sandbox: &RecordingSandbox) -> Vec<Option<Duration>> {
    sandbox
        .calls()
        .into_iter()
        .filter_map(|call| match call {
            Call::Execute(_, options) => Some(options.timeout),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn reaches_the_sandbox_when_the_model_omits_timeout() {
    let (_dir, sandbox, tool) = recording();
    tool.invoke(json!({ "command": "true" }), context(None))
        .await
        .unwrap();
    assert_eq!(timeouts(&sandbox), [Some(Duration::from_secs(300))]);
}

#[tokio::test]
async fn loses_to_an_explicit_timeout_from_the_model() {
    let (_dir, sandbox, tool) = recording();
    tool.invoke(json!({ "command": "true", "timeout": 7 }), context(None))
        .await
        .unwrap();
    assert_eq!(timeouts(&sandbox), [Some(Duration::from_secs(7))]);
}

#[test]
fn rejects_a_non_positive_or_non_finite_default_at_construction() {
    let error = make_shell(MakeShellOptions {
        default_timeout: Some(Duration::ZERO),
        ..MakeShellOptions::default()
    })
    .unwrap_err();
    assert!(
        error
            .to_string()
            .starts_with("defaultTimeout must be a positive finite number")
    );
}

#[tokio::test]
async fn uses_the_agent_sandbox_when_unbound() {
    let dir = tempfile::tempdir().unwrap();
    let sandbox = RecordingSandbox::new(Arc::new(TestSandbox::new(dir.path())));
    let tool = make_shell(MakeShellOptions::default()).unwrap();
    let result = output(
        tool.invoke(
            json!({ "command": "echo agent" }),
            context(Some(Arc::clone(&sandbox) as _)),
        )
        .await
        .unwrap(),
    );
    assert_eq!(result.output, "agent\n");
    assert_eq!(sandbox.calls().len(), 1);
}
