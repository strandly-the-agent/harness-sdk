//! Executor behaviour, including the parts of
//! strands-ts/src/tools/executors/__tests__/executor.cancel-signal.test.ts that don't need
//! middleware (Phase 4): `uses the supplied signal between sequential tool executions` and
//! `does not retry after the supplied signal is aborted`.

use std::sync::atomic::{AtomicUsize, Ordering as AtomicOrdering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use super::*;
use crate::agent::{Agent, AgentConfig, InvokeOptions};
use crate::cancel::{CancelOptions, CancelSignal};
use crate::hooks::{AfterToolCallEvent, BeforeToolCallEvent};
use crate::testing::{MockModel, mock_tool, text_result};
use crate::tools::{FunctionTool, ToolSource};
use crate::types::{ContentBlock, StopReason, ToolResultContent};

fn tool_turn(model: &MockModel, calls: &[(&str, &str)]) {
    model.add_turn(
        calls
            .iter()
            .map(|(name, id)| ContentBlock::ToolUse(ToolUseBlock::new(*name, *id, json!({}))))
            .collect(),
    );
}

fn agent(model: MockModel, tools: Vec<ToolSource>, executor: ToolExecutor) -> Agent {
    Agent::new(AgentConfig {
        model: Some(model.into()),
        tools,
        printer: Some(false),
        tool_executor: Some(executor),
        ..Default::default()
    })
    .unwrap()
}

fn results(agent: &Agent) -> Vec<(String, String)> {
    agent
        .messages()
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => {
                let text = match result.content.first() {
                    Some(ToolResultContent::Text(text)) => text.text.clone(),
                    _ => String::new(),
                };
                Some((result.tool_use_id.clone(), text))
            }
            _ => None,
        })
        .collect()
}

fn sleeper(name: &str, millis: u64, log: &Arc<Mutex<Vec<String>>>) -> FunctionTool {
    let log = Arc::clone(log);
    let label = name.to_owned();
    FunctionTool::builder(name, "Sleeps").callback(move |_, _| {
        let log = Arc::clone(&log);
        let label = label.clone();
        async move {
            log.lock().unwrap().push(format!("start {label}"));
            tokio::time::sleep(Duration::from_millis(millis)).await;
            log.lock().unwrap().push(format!("end {label}"));
            Ok(json!(label))
        }
    })
}

#[tokio::test(start_paused = true)]
async fn concurrent_executor_runs_tools_side_by_side_and_keeps_result_order() {
    let model = MockModel::new();
    tool_turn(&model, &[("slow", "t1"), ("fast", "t2")]);
    model.add_turn(vec!["done".into()]);
    let log = Arc::new(Mutex::new(Vec::new()));
    let tools = crate::tool_list![sleeper("slow", 50, &log), sleeper("fast", 10, &log)];
    let mut agent = agent(model, tools, ToolExecutor::Concurrent);
    agent.invoke("go").await.unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        ["start slow", "start fast", "end fast", "end slow"]
    );
    assert_eq!(
        results(&agent),
        [("t1".into(), "slow".into()), ("t2".into(), "fast".into())]
    );
}

#[tokio::test(start_paused = true)]
async fn sequential_executor_runs_tools_in_order() {
    let model = MockModel::new();
    tool_turn(&model, &[("slow", "t1"), ("fast", "t2")]);
    model.add_turn(vec!["done".into()]);
    let log = Arc::new(Mutex::new(Vec::new()));
    let tools = crate::tool_list![sleeper("slow", 50, &log), sleeper("fast", 10, &log)];
    let mut agent = agent(model, tools, ToolExecutor::Sequential);
    agent.invoke("go").await.unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        ["start slow", "end slow", "start fast", "end fast"]
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_panicking_tool_becomes_an_error_result_and_the_loop_continues() {
    let model = MockModel::new();
    tool_turn(&model, &[("boom", "t1"), ("fine", "t2")]);
    model.add_turn(vec!["recovered".into()]);
    let boom = FunctionTool::builder("boom", "Panics").callback(|_, _| async {
        if true {
            panic!("tool exploded");
        }
        Ok(json!(null))
    });
    let tools = crate::tool_list![boom, mock_tool("fine", |ctx| text_result(ctx, "ok"))];
    let mut agent = agent(model, tools, ToolExecutor::Concurrent);
    let result = agent.invoke("go").await.unwrap();
    assert_eq!(result.to_string(), "recovered");
    assert_eq!(
        results(&agent),
        [("t1".into(), "tool exploded".into()), ("t2".into(), "ok".into())]
    );
}

#[tokio::test]
async fn an_unknown_tool_gets_the_ts_not_found_result() {
    let model = MockModel::new();
    tool_turn(&model, &[("missing", "t1")]);
    model.add_turn(vec!["ok".into()]);
    let mut agent = agent(model, Vec::new(), ToolExecutor::Concurrent);
    agent.invoke("go").await.unwrap();
    assert_eq!(
        results(&agent),
        [("t1".into(), "Tool 'missing' not found in registry".into())]
    );
}

#[tokio::test]
async fn a_hook_cancel_gives_an_error_result_without_running_the_tool() {
    let model = MockModel::new();
    tool_turn(&model, &[("shell", "t1")]);
    model.add_turn(vec!["ok".into()]);
    let ran = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ran);
    let shell = mock_tool("shell", move |ctx| {
        counter.fetch_add(1, AtomicOrdering::SeqCst);
        text_result(ctx, "ran")
    });
    let mut agent = agent(model, crate::tool_list![shell], ToolExecutor::Concurrent);
    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name == "shell" {
            event.set_cancel("shell is disabled");
        }
    });
    agent.invoke("go").await.unwrap();
    assert_eq!(ran.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(results(&agent), [("t1".into(), "shell is disabled".into())]);
}

#[tokio::test]
async fn a_hook_can_select_a_different_tool_and_retry_a_call() {
    let model = MockModel::new();
    tool_turn(&model, &[("first", "t1")]);
    model.add_turn(vec!["ok".into()]);
    let replacement: Arc<dyn crate::tools::Tool> =
        Arc::new(mock_tool("replacement", |ctx| text_result(ctx, "replaced")));
    let mut agent = agent(
        model,
        crate::tool_list![mock_tool("first", |ctx| text_result(ctx, "first"))],
        ToolExecutor::Sequential,
    );
    agent.add_hook(move |event: &mut BeforeToolCallEvent| event.set_selected_tool(Arc::clone(&replacement)));
    let attempts = Arc::new(AtomicUsize::new(0));
    let seen = Arc::clone(&attempts);
    agent.add_hook(move |event: &mut AfterToolCallEvent| {
        if seen.fetch_add(1, AtomicOrdering::SeqCst) == 0 {
            event.set_retry(true);
        }
    });
    agent.invoke("go").await.unwrap();
    assert_eq!(attempts.load(AtomicOrdering::SeqCst), 2);
    assert_eq!(results(&agent), [("t1".into(), "replaced".into())]);
}

#[tokio::test]
async fn uses_the_supplied_signal_between_sequential_tool_executions() {
    let model = MockModel::new();
    tool_turn(&model, &[("cancel", "t1"), ("second", "t2")]);
    model.add_turn(vec!["unused".into()]);
    let (signal, handle) = CancelSignal::new();
    let ran = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&ran);
    let canceller = mock_tool("cancel", move |ctx| {
        handle.cancel(CancelOptions::default());
        text_result(ctx, "cancelled the run")
    });
    let second = mock_tool("second", move |ctx| {
        counter.fetch_add(1, AtomicOrdering::SeqCst);
        text_result(ctx, "ran")
    });
    let mut agent = agent(
        model,
        crate::tool_list![canceller, second],
        ToolExecutor::Sequential,
    );
    let options = InvokeOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    };
    let result = agent.invoke_with("go", options).await.unwrap();
    assert_eq!(ran.load(AtomicOrdering::SeqCst), 0);
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
    assert_eq!(
        results(&agent),
        [
            ("t1".into(), "cancelled the run".into()),
            ("t2".into(), "Tool execution cancelled".into())
        ]
    );
}

#[tokio::test]
async fn does_not_retry_after_the_supplied_signal_is_aborted() {
    let model = MockModel::new();
    tool_turn(&model, &[("tool", "t1")]);
    model.add_turn(vec!["unused".into()]);
    let (signal, handle) = CancelSignal::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&calls);
    let tool = mock_tool("tool", move |ctx| {
        counter.fetch_add(1, AtomicOrdering::SeqCst);
        text_result(ctx, "ok")
    });
    let mut agent = agent(model, crate::tool_list![tool], ToolExecutor::Sequential);
    agent.add_hook(move |event: &mut AfterToolCallEvent| {
        handle.cancel(CancelOptions::default());
        event.set_retry(true);
    });
    let options = InvokeOptions {
        cancel_signal: Some(signal),
        ..Default::default()
    };
    agent.invoke_with("go", options).await.unwrap();
    assert_eq!(calls.load(AtomicOrdering::SeqCst), 1);
}

#[tokio::test]
async fn siblings_finish_before_an_interrupt_stops_the_batch() {
    let model = MockModel::new();
    tool_turn(&model, &[("ask", "t1"), ("work", "t2")]);
    let ask = mock_tool("ask", |ctx| match ctx.interrupt("approve") {
        Ok(_) => text_result(ctx, "approved"),
        Err(interrupt) => crate::ToolOutcome::Interrupt(interrupt),
    });
    let work = mock_tool("work", |ctx| text_result(ctx, "worked"));
    let mut agent = agent(model, crate::tool_list![ask, work], ToolExecutor::Concurrent);
    let result = agent.invoke("go").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    assert_eq!(result.interrupts().len(), 1);
    assert_eq!(result.interrupts()[0].id, "tool:t1:approve");
    let pending = agent
        .local
        .interrupt_state
        .lock()
        .unwrap()
        .pending_tool_execution
        .clone()
        .unwrap();
    assert_eq!(
        pending.completed_tool_results["t2"]["toolResult"]["content"],
        json!([{ "text": "worked" }])
    );
}
