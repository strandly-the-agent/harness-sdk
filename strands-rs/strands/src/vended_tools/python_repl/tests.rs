//! Ported from strands-ts/src/vended-tools/python-repl/__tests__/python-repl.test.node.ts

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::Notify;

use super::*;
use crate::sandbox::StreamType;
use crate::sandbox::test_sandbox::{NotifyOnOutput, local_env, manual_sleeper, never_sleeper};
use crate::tools::Tool;
use crate::vended_tools::test_support::{
    Call, ExecuteOptionsSeen, ExplodingSandbox, RecordingSandbox, cancellable_context, context,
};

fn recording(dir: &tempfile::TempDir) -> Arc<RecordingSandbox> {
    RecordingSandbox::new(Arc::new(local_env(dir.path(), never_sleeper())))
}

async fn run(
    tool: &FunctionTool,
    input: JsonValue,
    sandbox: Option<Arc<dyn Sandbox>>,
) -> Result<PythonReplOutput, ToolError> {
    let value = tool.invoke(input, context(sandbox)).await?;
    Ok(serde_json::from_value(value).unwrap())
}

#[tokio::test]
async fn uses_the_agent_sandbox_and_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let sandbox = recording(&dir);
    let result = run(
        &python_repl(),
        json!({ "code": "print('hi')" }),
        Some(Arc::clone(&sandbox) as _),
    )
    .await
    .unwrap();
    let seen = ExecuteOptionsSeen {
        timeout: Some(Duration::from_secs(120)),
        cancel_signal: true,
    };
    assert_eq!(
        sandbox.calls(),
        [Call::ExecuteCode("print('hi')".into(), "python3".into(), seen)]
    );
    assert_eq!(
        result,
        PythonReplOutput {
            output: "hi\n".into(),
            error: String::new(),
            exit_code: 0
        }
    );
}

#[tokio::test]
async fn prefers_a_bound_sandbox_and_forwards_language_and_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let bound = recording(&dir);
    let tool = make_python_repl(MakePythonReplOptions {
        language: Some("python3".into()),
        sandbox: Some(Arc::clone(&bound) as _),
        ..MakePythonReplOptions::default()
    })
    .unwrap();
    run(
        &tool,
        json!({ "code": "pass", "timeout": 7 }),
        Some(Arc::new(ExplodingSandbox)),
    )
    .await
    .unwrap();
    let Call::ExecuteCode(code, language, options) = &bound.calls()[0] else {
        panic!("expected execute_code");
    };
    assert_eq!((code.as_str(), language.as_str()), ("pass", "python3"));
    assert_eq!(options.timeout, Some(Duration::from_secs(7)));
}

#[tokio::test]
async fn rejects_a_non_positive_timeout() {
    let dir = tempfile::tempdir().unwrap();
    let sandbox = recording(&dir);
    let error = run(
        &python_repl(),
        json!({ "code": "pass", "timeout": 0 }),
        Some(Arc::clone(&sandbox) as _),
    )
    .await
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("timeout must be a positive number of seconds, got 0"),
        "{error}"
    );
    assert!(sandbox.calls().is_empty());
}

#[tokio::test]
async fn wraps_sandbox_error_as_python_repl_error() {
    let error = run(
        &python_repl(),
        json!({ "code": "pass" }),
        Some(Arc::new(ExplodingSandbox)),
    )
    .await
    .unwrap_err();
    assert_eq!(error.to_string(), "agent sandbox used despite bound sandbox");
}

#[tokio::test]
async fn reports_failure_via_error_and_non_zero_exit() {
    let dir = tempfile::tempdir().unwrap();
    for (code, expected) in [
        ("raise ValueError('bad')", "ValueError: bad"),
        ("input()", "EOFError"),
    ] {
        let sandbox: Arc<dyn Sandbox> = Arc::new(local_env(dir.path(), never_sleeper()));
        let result = run(&python_repl(), json!({ "code": code }), Some(sandbox))
            .await
            .unwrap();
        assert_ne!(result.exit_code, 0);
        assert!(result.error.contains(expected), "{}", result.error);
    }
}

#[tokio::test]
async fn does_not_persist_state_across_calls_but_files_do() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.json");
    let sandbox: Arc<dyn Sandbox> = Arc::new(local_env(dir.path(), never_sleeper()));
    let write = format!(
        "import json; x = 42; json.dump({{'x': x}}, open({:?}, 'w'))",
        path.display().to_string()
    );
    run(
        &python_repl(),
        json!({ "code": write }),
        Some(Arc::clone(&sandbox)),
    )
    .await
    .unwrap();
    let missing = run(
        &python_repl(),
        json!({ "code": "print(x)" }),
        Some(Arc::clone(&sandbox)),
    )
    .await
    .unwrap();
    assert!(missing.error.contains("NameError"));
    let read = format!(
        "import json; print(json.load(open({:?}))['x'])",
        path.display().to_string()
    );
    let restored = run(&python_repl(), json!({ "code": read }), Some(sandbox))
        .await
        .unwrap();
    assert_eq!(
        restored,
        PythonReplOutput {
            output: "42\n".into(),
            error: String::new(),
            exit_code: 0
        }
    );
}

#[tokio::test]
async fn timeout_carries_partial_output_with_the_success_field_names() {
    let dir = tempfile::tempdir().unwrap();
    let notify = Arc::new(Notify::new());
    let sleeper = manual_sleeper(Arc::clone(&notify), Arc::new(Mutex::new(Vec::new())));
    let sandbox = NotifyOnOutput {
        inner: local_env(dir.path(), sleeper),
        notify,
        expected: vec![(StreamType::Stdout, "partial\n")],
    };
    let code = "print('partial', flush=True)\nimport time; time.sleep(10)";
    let error = run(
        &python_repl(),
        json!({ "code": code, "timeout": 0.5 }),
        Some(Arc::new(sandbox)),
    )
    .await
    .unwrap_err()
    .to_string();
    let (head, payload) = error.split_once('\n').unwrap();
    assert_eq!(head, "Execution timed out after 0.5 seconds");
    assert_eq!(
        serde_json::from_str::<JsonValue>(payload).unwrap(),
        json!({ "output": "partial\n", "error": "", "exit_code": 124 })
    );
}

#[tokio::test]
async fn cancellation_stops_the_call_and_propagates_unwrapped() {
    let dir = tempfile::tempdir().unwrap();
    let sandbox: Arc<dyn Sandbox> = Arc::new(local_env(dir.path(), never_sleeper()));
    let (ctx, handle) = cancellable_context(Some(sandbox));
    let tool = python_repl();
    let call = tool.invoke(json!({ "code": "import time; time.sleep(10)" }), ctx);
    let (result, ()) = tokio::join!(call, async {
        tokio::task::yield_now().await;
        handle.cancel(crate::CancelOptions::default());
    });
    assert_eq!(result.unwrap_err().to_string(), "Execution aborted");
}

#[test]
fn rejects_invalid_arguments() {
    let empty = make_python_repl(MakePythonReplOptions {
        name: Some(String::new()),
        ..Default::default()
    });
    assert!(empty.unwrap_err().to_string().contains("name"));
    let bad = make_python_repl(MakePythonReplOptions {
        language: Some("python3; rm -rf /".into()),
        ..Default::default()
    });
    assert_eq!(
        bad.unwrap_err().to_string(),
        "language contains invalid characters: python3; rm -rf /"
    );
}

#[test]
fn has_the_default_tool_spec_and_exports() {
    let tool = python_repl();
    assert_eq!(tool.name(), "python_repl");
    assert_eq!(tool.tool_spec().description(), PYTHON_REPL_DESCRIPTION);
    assert_eq!(tool.tool_spec().input_schema(), Some(&python_repl_input_schema()));
}

#[test]
fn resolves_both_overloads_and_applies_a_custom_name_and_description() {
    let tool = make_python_repl(MakePythonReplOptions {
        name: Some("run_python".into()),
        description: Some("custom".into()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(
        (tool.name(), tool.tool_spec().description()),
        ("run_python", "custom")
    );
    let bound = make_python_repl(MakePythonReplOptions {
        sandbox: Some(crate::vended_tools::test_support::local()),
        ..Default::default()
    })
    .unwrap();
    assert_eq!(bound.name(), "python_repl");
}
