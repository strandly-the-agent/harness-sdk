//! Ported from strands-ts/src/vended-tools/bash/__tests__/bash.test.node.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use serde_json::json;

use super::*;
use crate::sandbox::test_sandbox::instant_sleeper;
use crate::tools::Tool;
use crate::vended_tools::test_support::context;

fn tool(dir: &tempfile::TempDir) -> FunctionTool {
    bash_with(Some(dir.path().to_path_buf()), tokio_sleeper())
}

async fn execute(tool: &FunctionTool, context: &ToolContext, command: &str) -> BashOutput {
    let value = tool
        .invoke(json!({ "mode": "execute", "command": command }), context.clone())
        .await
        .unwrap();
    serde_json::from_value(value).unwrap()
}

async fn invoke_err(tool: &FunctionTool, input: JsonValue) -> String {
    tool.invoke(input, context(None)).await.unwrap_err().to_string()
}

#[tokio::test]
async fn accepts_valid_execute_command() {
    let dir = tempfile::tempdir().unwrap();
    let result = tool(&dir)
        .invoke(
            json!({ "mode": "execute", "command": "echo \"test\"" }),
            context(None),
        )
        .await
        .unwrap();
    assert!(result.get("output").is_some());
    assert!(result.get("error").is_some());
}

#[tokio::test]
async fn accepts_valid_restart_command() {
    let dir = tempfile::tempdir().unwrap();
    let result = tool(&dir)
        .invoke(json!({ "mode": "restart" }), context(None))
        .await
        .unwrap();
    assert_eq!(result, json!("Bash session restarted"));
}

#[tokio::test]
async fn rejects_invalid_mode() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        tool(&dir)
            .invoke(json!({ "mode": "invalid" }), context(None))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn rejects_execute_without_command() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        tool(&dir)
            .invoke(json!({ "mode": "execute" }), context(None))
            .await
            .is_err()
    );
}

#[tokio::test]
async fn accepts_valid_timeout_configuration() {
    let dir = tempfile::tempdir().unwrap();
    let input = json!({ "mode": "execute", "command": "echo \"fast\"", "timeout": 300 });
    assert!(
        tool(&dir)
            .invoke(input, context(None))
            .await
            .unwrap()
            .get("output")
            .is_some()
    );
}

#[tokio::test]
async fn rejects_negative_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let input = json!({ "mode": "execute", "command": "echo test", "timeout": -1 });
    assert!(tool(&dir).invoke(input, context(None)).await.is_err());
}

#[tokio::test]
async fn creates_session_on_first_execute() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        execute(&tool(&dir), &context(None), "echo \"test\"")
            .await
            .output
            .contains("test")
    );
}

#[tokio::test]
async fn creates_new_session_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    execute(&tool, &ctx, "TEST_RESTART=\"exists\"").await;
    assert_eq!(
        tool.invoke(json!({ "mode": "restart" }), ctx.clone())
            .await
            .unwrap(),
        json!("Bash session restarted")
    );
    assert!(
        !execute(&tool, &ctx, "echo $TEST_RESTART")
            .await
            .output
            .contains("exists")
    );
}

#[tokio::test]
async fn restarts_existing_session_when_restart_is_called() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    execute(&tool, &ctx, "TEST_VAR=\"initial\"").await;
    tool.invoke(json!({ "mode": "restart" }), ctx.clone())
        .await
        .unwrap();
    assert_eq!(
        execute(&tool, &ctx, "echo \"${TEST_VAR:-empty}\"").await.output,
        "empty"
    );
}

#[tokio::test]
async fn persists_environment_variables_between_calls() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    execute(&tool, &ctx, "export MY_VAR=\"persistent_value\"").await;
    assert_eq!(
        execute(&tool, &ctx, "echo $MY_VAR").await.output,
        "persistent_value"
    );
}

#[tokio::test]
async fn persists_working_directory_between_calls() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    execute(&tool, &ctx, "cd /tmp").await;
    let out = execute(&tool, &ctx, "pwd").await.output;
    assert_eq!(
        std::fs::canonicalize(out).unwrap(),
        std::fs::canonicalize("/tmp").unwrap()
    );
}

#[tokio::test]
async fn provides_isolated_sessions_for_different_agents() {
    let dir = tempfile::tempdir().unwrap();
    let tool = tool(&dir);
    execute(&tool, &context(None), "AGENT_VAR=\"agent1\"").await;
    assert!(
        !execute(&tool, &context(None), "echo $AGENT_VAR")
            .await
            .output
            .contains("agent1")
    );
}

#[tokio::test]
async fn handles_session_restart_with_no_existing_session_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    assert_eq!(
        tool.invoke(json!({ "mode": "restart" }), ctx.clone())
            .await
            .unwrap(),
        json!("Bash session restarted")
    );
    assert_eq!(execute(&tool, &ctx, "echo \"works\"").await.output, "works");
}

#[tokio::test]
async fn executes_command_and_returns_output() {
    let dir = tempfile::tempdir().unwrap();
    let result = execute(&tool(&dir), &context(None), "echo \"Hello World\"").await;
    assert!(result.output.contains("Hello World"));
    assert_eq!(result.error, "");
}

#[tokio::test]
async fn returns_empty_stderr_on_success() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        execute(&tool(&dir), &context(None), "echo \"success\"")
            .await
            .error,
        ""
    );
}

#[tokio::test]
async fn captures_stderr_on_command_error() {
    let dir = tempfile::tempdir().unwrap();
    let result = execute(&tool(&dir), &context(None), "nonexistent_command_xyz").await;
    assert!(result.error.contains("not found"), "{}", result.error);
}

#[tokio::test]
async fn completes_command_before_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let input = json!({ "mode": "execute", "command": "echo \"fast\"", "timeout": 5 });
    let result = tool(&dir).invoke(input, context(None)).await.unwrap();
    assert!(result["output"].as_str().unwrap().contains("fast"));
}

#[tokio::test]
async fn throws_bash_timeout_error_when_command_times_out() {
    let dir = tempfile::tempdir().unwrap();
    let tool = bash_with(Some(dir.path().to_path_buf()), instant_sleeper());
    let error = invoke_err(
        &tool,
        json!({ "mode": "execute", "command": "sleep 10", "timeout": 0.1 }),
    )
    .await;
    assert_eq!(error, "Command timed out after 0.1 seconds");
}

#[tokio::test]
async fn uses_default_timeout_of_120_seconds() {
    let dir = tempfile::tempdir().unwrap();
    let delays = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let recorded = std::sync::Arc::clone(&delays);
    let sleeper: Sleeper = std::sync::Arc::new(move |delay| {
        recorded.lock().unwrap().push(delay);
        Box::pin(futures::future::pending())
    });
    let tool = bash_with(Some(dir.path().to_path_buf()), sleeper);
    assert!(
        execute(&tool, &context(None), "echo \"test\"")
            .await
            .output
            .contains("test")
    );
    assert_eq!(*delays.lock().unwrap(), [Duration::from_secs(120)]);
}

#[tokio::test]
async fn respects_custom_timeout_for_new_session() {
    let dir = tempfile::tempdir().unwrap();
    let input = json!({ "mode": "execute", "command": "echo \"custom\"", "timeout": 10 });
    let result = tool(&dir).invoke(input, context(None)).await.unwrap();
    assert!(result["output"].as_str().unwrap().contains("custom"));
}

#[tokio::test]
async fn handles_timeout_during_command_with_large_output() {
    let dir = tempfile::tempdir().unwrap();
    let tool = bash_with(Some(dir.path().to_path_buf()), instant_sleeper());
    let input = json!({ "mode": "execute", "command": "while true; do echo \"spam\"; done", "timeout": 0.1 });
    assert!(
        invoke_err(&tool, input)
            .await
            .starts_with("Command timed out after")
    );
}

#[tokio::test]
async fn validates_command_is_required_for_execute_mode() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        invoke_err(&tool(&dir), json!({ "mode": "execute" })).await,
        "command is required when mode is \"execute\""
    );
}

#[tokio::test]
async fn validates_command_is_required_with_undefined_command() {
    let dir = tempfile::tempdir().unwrap();
    let error = invoke_err(&tool(&dir), json!({ "mode": "execute", "command": null })).await;
    assert!(!error.is_empty());
}

#[tokio::test]
async fn validates_command_is_required_with_empty_string() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        invoke_err(&tool(&dir), json!({ "mode": "execute", "command": "" })).await,
        "command is required when mode is \"execute\""
    );
}

#[tokio::test]
async fn handles_command_execution_in_a_session_without_proper_initialization() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    execute(&tool, &ctx, "echo \"init\"").await;
    tool.invoke(json!({ "mode": "restart" }), ctx.clone())
        .await
        .unwrap();
    assert!(
        execute(&tool, &ctx, "echo \"after restart\"")
            .await
            .output
            .contains("after restart")
    );
}

#[tokio::test]
async fn properly_cleans_up_session_on_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    execute(&tool, &ctx, "CLEANUP_TEST=\"should_be_gone\"").await;
    tool.invoke(json!({ "mode": "restart" }), ctx.clone())
        .await
        .unwrap();
    assert_eq!(
        execute(&tool, &ctx, "echo \"${CLEANUP_TEST:-empty}\"")
            .await
            .output,
        "empty"
    );
}

#[tokio::test]
async fn handles_multiple_restarts_in_sequence() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    for _ in 0..2 {
        assert_eq!(
            tool.invoke(json!({ "mode": "restart" }), ctx.clone())
                .await
                .unwrap(),
            json!("Bash session restarted")
        );
    }
    assert!(
        execute(&tool, &ctx, "echo \"still works\"")
            .await
            .output
            .contains("still works")
    );
}

#[tokio::test]
async fn handles_command_with_empty_output_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let result = execute(&tool(&dir), &context(None), "true").await;
    assert_eq!((result.output.as_str(), result.error.as_str()), ("", ""));
}

#[tokio::test]
async fn handles_command_with_only_whitespace_output() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(
        execute(&tool(&dir), &context(None), "echo \"   \"").await.output,
        ""
    );
}

#[tokio::test]
async fn handles_very_long_command_output() {
    let dir = tempfile::tempdir().unwrap();
    let out = execute(
        &tool(&dir),
        &context(None),
        "for i in {1..100}; do echo \"Line $i of output\"; done",
    )
    .await
    .output;
    assert!(out.contains("Line 1 of output"));
    assert!(out.contains("Line 100 of output"));
}

#[tokio::test]
async fn starts_in_process_cwd() {
    let dir = tempfile::tempdir().unwrap();
    let out = execute(&tool(&dir), &context(None), "pwd").await.output;
    assert_eq!(
        std::fs::canonicalize(out).unwrap(),
        std::fs::canonicalize(dir.path()).unwrap()
    );
}

#[test]
fn has_correct_tool_name() {
    assert_eq!(bash().name(), "bash");
}

#[test]
fn has_description() {
    assert_eq!(bash().description(), DEFAULT_DESCRIPTION);
}

#[test]
fn has_tool_spec() {
    let tool = bash();
    assert_eq!(tool.tool_spec().name(), "bash");
    assert_eq!(tool.tool_spec().input_schema(), Some(&bash_input_schema()));
}

#[test]
fn bash_timeout_error_has_correct_properties() {
    let timeout = BashTimeoutError::new("timeout message");
    assert_eq!(
        (BashTimeoutError::NAME, timeout.to_string().as_str()),
        ("BashTimeoutError", "timeout message")
    );
    let _: &dyn std::error::Error = &timeout;
}

#[test]
fn bash_session_error_has_correct_properties() {
    let session = BashSessionError::new("session error message");
    assert_eq!(
        (BashSessionError::NAME, session.to_string().as_str()),
        ("BashSessionError", "session error message")
    );
    let _: &dyn std::error::Error = &session;
}

#[tokio::test]
async fn handles_process_close_during_command_execution() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    let error = tool
        .invoke(json!({ "mode": "execute", "command": "exit 0" }), ctx.clone())
        .await
        .unwrap_err();
    assert_eq!(error.to_string(), "Bash process exited unexpectedly with code 0");
    assert!(
        execute(&tool, &ctx, "echo \"new session\"")
            .await
            .output
            .contains("new session")
    );
}

#[tokio::test]
async fn creates_new_session_when_none_exists() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        execute(&tool(&dir), &context(None), "echo \"first\"")
            .await
            .output
            .contains("first")
    );
}

#[tokio::test]
async fn handles_restart_when_no_session_exists() {
    let dir = tempfile::tempdir().unwrap();
    let result = tool(&dir)
        .invoke(json!({ "mode": "restart" }), context(None))
        .await
        .unwrap();
    assert_eq!(result, json!("Bash session restarted"));
}

#[tokio::test]
async fn creates_session_with_default_timeout_when_not_specified() {
    let dir = tempfile::tempdir().unwrap();
    assert!(
        execute(&tool(&dir), &context(None), "echo \"default\"")
            .await
            .output
            .contains("default")
    );
}

#[tokio::test]
async fn timeout_still_applies_after_the_command_closes_stdout() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    let input = json!({ "mode": "execute", "command": "exec >&-", "timeout": 0.5 });
    let call = tokio::time::timeout(Duration::from_secs(10), tool.invoke(input, ctx.clone()));
    let error = call.await.expect("bash call ignored its timeout").unwrap_err();
    assert_eq!(error.to_string(), "Command timed out after 0.5 seconds");
    assert_eq!(execute(&tool, &ctx, "echo \"next\"").await.output, "next");
}

#[tokio::test]
async fn dropped_call_does_not_leak_its_output_into_the_next_call() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    let input = json!({ "mode": "execute", "command": "sleep 1; echo first" });
    let dropped = tokio::time::timeout(Duration::from_millis(300), tool.invoke(input, ctx.clone())).await;
    assert!(dropped.is_err(), "the first call should still be running");
    assert_eq!(execute(&tool, &ctx, "echo second").await.output, "second");
}

#[tokio::test]
async fn restart_stops_a_running_command() {
    let dir = tempfile::tempdir().unwrap();
    let (tool, ctx) = (tool(&dir), context(None));
    execute(&tool, &ctx, "RESTART_TEST=set").await;
    let running = tool.invoke(json!({ "mode": "execute", "command": "sleep 5" }), ctx.clone());
    let queued = tool.invoke(
        json!({ "mode": "execute", "command": "echo queued" }),
        ctx.clone(),
    );
    let restart = tool.invoke(json!({ "mode": "restart" }), ctx.clone());
    let started = std::time::Instant::now();

    let (running, queued, restart) = tokio::join!(running, queued, restart);

    assert!(
        started.elapsed() < Duration::from_secs(4),
        "restart waited for the command"
    );
    assert_eq!(restart.unwrap(), json!("Bash session restarted"));
    let killed = "Bash process exited unexpectedly with code unknown";
    assert_eq!(running.unwrap_err().to_string(), killed);
    // A call queued on the replaced session fails with it, as its TS listeners would.
    assert_eq!(queued.unwrap_err().to_string(), killed);
    assert_eq!(
        execute(&tool, &ctx, "echo \"${RESTART_TEST:-empty}\"")
            .await
            .output,
        "empty"
    );
}
