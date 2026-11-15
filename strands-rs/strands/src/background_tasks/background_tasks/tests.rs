//! Ported from strands-ts/src/background-tasks/__tests__/background-tasks.test.ts.

use std::sync::{Arc, Mutex};

use serde_json::json;

use super::*;
use crate::agent::Agent;
use crate::agent::TakeSnapshotOptions;
use crate::background_tasks::BackgroundTasksConfig;
use crate::cancel::CancelOptions;
use crate::hooks::{AfterToolCallEvent, BeforeToolCallEvent};
use crate::interrupt::InterruptParams;
use crate::middleware::{ExecuteToolStage, InvokeModelStage, MiddlewareStage};
use crate::testing::MockMessageModel;
use crate::tools::{FunctionTool, ToolError};
use crate::types::messages::StopReason;

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn call(name: &str, id: &str, input: JsonValue) -> ContentBlock {
    ToolUseBlock::new(name, id, input).into()
}

fn model(turns: Vec<Vec<ContentBlock>>) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    for turn in turns {
        model.add_turn(turn);
    }
    model
}

fn agentic(names: &[&str]) -> BackgroundTasksConfig {
    BackgroundTasksConfig {
        agentic: Some(names.iter().map(|name| (*name).to_owned()).collect()),
        ..BackgroundTasksConfig::default()
    }
}

fn work_tool(name: &str) -> FunctionTool {
    FunctionTool::new(name, "Perform work.", |input: JsonValue, _ctx| async move {
        Ok::<_, ToolError>(json!(format!(
            "done:{}",
            input["value"].as_str().unwrap_or_default()
        )))
    })
}

fn agent(model: &Arc<MockMessageModel>, tools: Vec<FunctionTool>, config: BackgroundTasksConfig) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .tools(tools)
        .background_tasks(config)
        .printer(false)
        .build()
        .unwrap()
}

/// A tool that finishes once `gate` is notified.
fn gated_tool(name: &str, gate: &Arc<tokio::sync::Notify>) -> FunctionTool {
    let gate = Arc::clone(gate);
    FunctionTool::new(name, "Wait for the gate.", move |_input: JsonValue, _ctx| {
        let gate = Arc::clone(&gate);
        async move {
            gate.notified().await;
            Ok::<_, ToolError>(json!("released"))
        }
    })
}

fn deliveries(agent: &Agent) -> Vec<ToolUseBlock> {
    agent
        .messages()
        .iter()
        .flat_map(Message::content)
        .filter_map(|block| match block {
            ContentBlock::ToolUse(tool_use) if tool_use.name() == MANAGE_TOOL_NAME => Some(tool_use.clone()),
            _ => None,
        })
        .collect()
}

fn result_for<'a>(agent: &'a Agent, id: &str) -> &'a ToolResultBlock {
    agent
        .messages()
        .iter()
        .flat_map(Message::content)
        .find_map(|block| match block {
            ContentBlock::ToolResult(result) if result.tool_use_id() == id => Some(result),
            _ => None,
        })
        .unwrap()
}

/// The task JSON of a delivered result, without its timestamps.
fn delivered_task(result: &ToolResultBlock) -> JsonValue {
    let ToolResultContent::Json(block) = &result.content()[0] else {
        panic!("expected the task JSON first");
    };
    let mut task = block.json().clone();
    let object = task.as_object_mut().unwrap();
    assert!(object.remove("createdAt").unwrap().is_string());
    assert!(object.remove("lastUpdatedAt").unwrap().is_string());
    task
}

#[tokio::test]
async fn dispatches_selected_calls_through_the_normal_tool_pipeline_and_delivers_their_results() {
    let model = model(vec![
        vec![call(
            "work",
            "work-use",
            json!({"value": "background", "_background_execution": true}),
        )],
        vec![text("Tasks admitted.")],
        vec![text("Result received.")],
    ]);
    let mut agent = agent(&model, vec![work_tool("work")], agentic(&["work"]));
    let specs: Arc<Mutex<Option<Vec<ToolSpec>>>> = Arc::default();
    let slot = Arc::clone(&specs);
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        slot.lock()
            .unwrap()
            .get_or_insert_with(|| context.tool_specs.clone());
        next.run(context)
    }));
    let retried = Arc::new(Mutex::new(false));
    agent.add_hook(move |event: &mut AfterToolCallEvent| {
        let mut retried = retried.lock().unwrap();
        if !*retried {
            *retried = true;
            event.set_retry(true);
        }
    });
    let calls = Arc::new(Mutex::new(0));
    let counter = Arc::clone(&calls);
    agent.add_middleware(ExecuteToolStage::wrap(move |context, next| {
        *counter.lock().unwrap() += 1;
        next.run(context)
    }));
    agent.invoke("Run work.").await.unwrap();
    assert_eq!(*calls.lock().unwrap(), 2);
    let specs = specs.lock().unwrap().clone().unwrap();
    let work = specs.iter().find(|spec| spec.name() == "work").unwrap();
    assert!(
        work.input_schema().unwrap()["properties"]
            .get(BACKGROUND_PROPERTY)
            .is_some()
    );
    assert!(specs.iter().any(|spec| spec.name() == MANAGE_TOOL_NAME));
    let delivery = deliveries(&agent).remove(0);
    assert_eq!(
        delivery.input(),
        &json!({"mode": "get", "taskId": delivery.tool_use_id()})
    );
    let result = result_for(&agent, delivery.tool_use_id());
    assert_eq!(*result.status(), ToolResultStatus::Success);
    assert_eq!(
        delivered_task(result),
        json!({"taskId": delivery.tool_use_id(), "toolUseId": "work-use", "toolName": "work", "status": "completed"})
    );
    assert_eq!(result.content()[1], ToolResultContent::from("done:background"));
}

#[tokio::test]
async fn delivers_s_aborterror_and_finishes_the_invocation() {
    let cancelling = FunctionTool::new(
        "work",
        "Await an aborted operation.",
        |_input: JsonValue, ctx: ToolContext| {
            ctx.agent().cancel(CancelOptions::default());
            async { Err::<JsonValue, _>(ToolError::msg("aborted")) }
        },
    );
    let model = model(vec![
        vec![call("work", "work-use", json!({"_background_execution": true}))],
        vec![text("Task admitted.")],
        vec![text("Cancellation received.")],
    ]);
    let mut agent = agent(&model, vec![cancelling], agentic(&["work"]));
    let result = agent.invoke("Run work.").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let found = deliveries(&agent);
    assert_eq!(found.len(), 1);
    let delivered = result_for(&agent, found[0].tool_use_id());
    assert_eq!(delivered.content().len(), 1);
    assert_eq!(
        delivered_task(delivered),
        json!({"taskId": found[0].tool_use_id(), "toolUseId": "work-use", "toolName": "work", "status": "cancelled"})
    );
    assert_eq!(agent.app_state().get_value(BACKGROUND_TASKS_STATE_KEY), None);
}

#[tokio::test]
async fn applies_always_and_never_policies_per_tool() {
    let executions: Arc<Mutex<Vec<(String, JsonValue)>>> = Arc::default();
    let recording = |name: &'static str| {
        let log = Arc::clone(&executions);
        FunctionTool::new(name, "Run.", move |input: JsonValue, _ctx| {
            log.lock().unwrap().push((name.to_owned(), input));
            async { Ok::<_, ToolError>(json!("ok")) }
        })
    };
    let model = model(vec![
        vec![
            call(
                "foreground",
                "background-use",
                json!({"_background_execution": false}),
            ),
            call(
                "foreground",
                "foreground-use",
                json!({"_background_execution": true}),
            ),
        ],
        vec![text("Task admitted.")],
        vec![text("Result received.")],
    ]);
    let config = BackgroundTasksConfig {
        always: vec!["background".to_owned()],
        never: vec!["*".to_owned()],
        ..BackgroundTasksConfig::default()
    };
    let mut agent = agent(
        &model,
        vec![recording("background"), recording("foreground")],
        config,
    );
    let specs: Arc<Mutex<Option<Vec<ToolSpec>>>> = Arc::default();
    let slot = Arc::clone(&specs);
    agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
        slot.lock()
            .unwrap()
            .get_or_insert_with(|| context.tool_specs.clone());
        next.run(context)
    }));
    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().tool_use_id() == "background-use" {
            let renamed = ToolUse::new("background", "background-use", event.tool_use().input().clone());
            event.set_tool_use(renamed);
        }
    });
    agent.invoke("Run both.").await.unwrap();
    let mut ran: Vec<(String, JsonValue)> = executions.lock().unwrap().clone();
    ran.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        ran,
        [
            ("background".to_owned(), json!({})),
            ("foreground".to_owned(), json!({}))
        ]
    );
    assert_eq!(deliveries(&agent).len(), 1);
    for spec in specs.lock().unwrap().clone().unwrap() {
        let properties = spec
            .input_schema()
            .and_then(|schema| schema.get("properties"))
            .cloned();
        assert!(properties.is_none_or(|properties| properties.get(BACKGROUND_PROPERTY).is_none()));
    }
}

#[test]
fn rejects_conflicting_tool_policies() {
    let config = BackgroundTasksConfig {
        always: vec!["work".to_owned()],
        never: vec!["work".to_owned()],
        ..BackgroundTasksConfig::default()
    };
    let error = Agent::builder()
        .model(MockMessageModel::new())
        .tools([work_tool("work")])
        .background_tasks(config)
        .build()
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "Tool 'work' cannot be configured as both 'always' and 'never'"
    );
}

#[tokio::test]
async fn rejects_an_exact_tool_that_cannot_run_in_the_background() {
    let config = BackgroundTasksConfig {
        always: vec!["summarize_context".to_owned()],
        ..BackgroundTasksConfig::default()
    };
    let model = model(vec![vec![text("hi")]]);
    let mut agent = agent(&model, vec![work_tool("summarize_context")], config);
    let error = agent.invoke("hi").await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "Tool 'summarize_context' cannot run in the background"
    );
}

#[tokio::test]
async fn runs_queued_work_under_the_request_that_submitted_it_even_after_that_request_returned() {
    let seen: Arc<Mutex<Option<bool>>> = Arc::default();
    let slot = Arc::clone(&seen);
    let probe = FunctionTool::new(
        "probe",
        "Record the request.",
        move |_input: JsonValue, ctx: ToolContext| {
            *slot.lock().unwrap() = Some(ctx.invocation_context().is_some());
            async { Ok::<_, ToolError>(json!("probed")) }
        },
    );
    let model = model(vec![
        vec![call("probe", "probe-use", json!({"_background_execution": true}))],
        vec![text("Tasks admitted.")],
        vec![text("Result received.")],
    ]);
    let config = BackgroundTasksConfig {
        wait_for_completion: false,
        ..agentic(&["probe"])
    };
    let mut agent = agent(&model, vec![probe], config);
    agent.invoke("Run work.").await.unwrap();
    let manager = &agent
        .background_tasks
        .as_ref()
        .unwrap()
        .attached()
        .unwrap()
        .manager;
    manager.wait_for_idle(None).await.unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(true));
}

#[tokio::test]
async fn delivers_work_that_finishes_between_invocations() {
    let model = model(vec![
        vec![call(
            "work",
            "work-use",
            json!({"value": "later", "_background_execution": true}),
        )],
        vec![text("Tasks admitted.")],
        vec![text("Delivered.")],
    ]);
    let config = BackgroundTasksConfig {
        wait_for_completion: false,
        ..agentic(&["work"])
    };
    let gate = Arc::new(tokio::sync::Notify::new());
    let mut agent = agent(&model, vec![gated_tool("work", &gate)], config);
    agent.invoke("Run work.").await.unwrap();
    assert!(deliveries(&agent).is_empty());
    gate.notify_one();
    let manager = agent
        .background_tasks
        .as_ref()
        .unwrap()
        .attached()
        .unwrap()
        .manager
        .clone();
    manager.wait_for_idle(None).await.unwrap();
    let persisted: Vec<BackgroundTask> = agent
        .app_state()
        .get(BACKGROUND_TASKS_STATE_KEY)
        .unwrap()
        .unwrap();
    assert_eq!(persisted[0].status, TaskStatus::Completed);
    agent.invoke("Next.").await.unwrap();
    assert_eq!(deliveries(&agent).len(), 1);
    assert_eq!(agent.app_state().get_value(BACKGROUND_TASKS_STATE_KEY), None);
    assert!(!manager.has_tasks());
}

#[tokio::test]
async fn stops_waiting_for_running_tasks_when_the_invocation_is_cancelled() {
    let started = Arc::new(tokio::sync::Notify::new());
    let notify = Arc::clone(&started);
    let hang = FunctionTool::new(
        "hang",
        "Wait forever.",
        move |_input: JsonValue, ctx: ToolContext| {
            notify.notify_one();
            async move {
                ctx.cancel_signal().cancelled().await;
                Ok::<_, ToolError>(json!("stopped"))
            }
        },
    );
    let model = model(vec![
        vec![call("hang", "hang-use", json!({"_background_execution": true}))],
        vec![text("Task admitted.")],
    ]);
    let mut agent = agent(&model, vec![hang], agentic(&["hang"]));
    let handle = agent.cancel_handle();
    let canceller = tokio::spawn(async move {
        started.notified().await;
        // Let the invocation reach its `AfterInvocationEvent` wait.
        for _ in 0..50 {
            tokio::task::yield_now().await;
        }
        handle.cancel(CancelOptions::default());
    });
    let result = agent.invoke("Run.").await.unwrap();
    canceller.await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(deliveries(&agent).is_empty());
}

#[tokio::test]
async fn fails_interrupted_work_restored_from_appstate() {
    let stored = json!([{
        "taskId": "task-1",
        "toolUseId": "work-use",
        "toolName": "work",
        "status": "input_required",
        "createdAt": "2026-01-01T00:00:00.000Z",
        "lastUpdatedAt": "2026-01-01T00:00:00.000Z",
        "interrupts": [{"id": "tool:task-1:approve", "name": "approve", "source": "tool"}]
    }]);
    let mut app_state = crate::types::json::JsonObject::new();
    app_state.insert(BACKGROUND_TASKS_STATE_KEY.to_owned(), stored);
    let model = model(vec![vec![text("Delivered.")]]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools([work_tool("work")])
        .app_state(app_state)
        .background_tasks(true)
        .printer(false)
        .build()
        .unwrap();
    agent.initialize().await.unwrap();
    let persisted: Vec<BackgroundTask> = agent
        .app_state()
        .get(BACKGROUND_TASKS_STATE_KEY)
        .unwrap()
        .unwrap();
    assert_eq!(persisted[0].status, TaskStatus::Failed);
    assert_eq!(persisted[0].interrupts, None);
    assert_eq!(
        persisted[0].error,
        Some(Failure::new(
            FailureType::ExecutionError,
            "Background task cannot resume after restoring persisted state"
        ))
    );
    agent.invoke("Next.").await.unwrap();
    let delivered = deliveries(&agent);
    assert_eq!(delivered.len(), 1);
    assert_eq!(delivered[0].tool_use_id(), "task-1");
}

#[tokio::test]
async fn surfaces_and_resumes_interrupts_from_detached_tools() {
    let gated = FunctionTool::new(
        "work",
        "Ask first.",
        |_input: JsonValue, ctx: ToolContext| async move {
            let answer = ctx.interrupt(InterruptParams::new("approve"))?;
            Ok::<_, ToolError>(json!(format!("approved:{}", answer.as_str().unwrap_or_default())))
        },
    );
    let model = model(vec![
        vec![call("work", "work-use", json!({"_background_execution": true}))],
        vec![text("Task admitted.")],
        vec![text("Resumed.")],
        vec![text("Delivered.")],
    ]);
    let mut agent = agent(&model, vec![gated], agentic(&["work"]));
    let interrupted = agent.invoke("Run work.").await.unwrap();
    assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
    let interrupt = interrupted.interrupts()[0].clone();
    assert!(interrupt.id().starts_with("tool:") && interrupt.id().ends_with(":approve"));
    let result = agent
        .invoke(vec![InterruptResponse::new(interrupt.id(), json!("yes"))])
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let found = deliveries(&agent);
    assert_eq!(found.len(), 1);
    let delivered = result_for(&agent, found[0].tool_use_id());
    assert_eq!(delivered.content()[1], ToolResultContent::from("approved:yes"));
}

#[tokio::test]
async fn cannot_load_a_snapshot_while_background_tasks_are_still_tracked() {
    let model = model(vec![
        vec![call(
            "work",
            "work-use",
            json!({"value": "x", "_background_execution": true}),
        )],
        vec![text("Tasks admitted.")],
    ]);
    let config = BackgroundTasksConfig {
        wait_for_completion: false,
        ..agentic(&["work"])
    };
    let gate = Arc::new(tokio::sync::Notify::new());
    let mut agent = agent(&model, vec![gated_tool("work", &gate)], config);
    agent.invoke("Run work.").await.unwrap();
    let snapshot = agent.take_snapshot(&TakeSnapshotOptions::session()).unwrap();
    assert_eq!(
        agent.load_snapshot(&snapshot).unwrap_err().to_string(),
        "Cannot load a snapshot while background tasks are still tracked"
    );
}
