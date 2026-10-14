//! Background-task dispatch and delivery through the public API (`background-tasks.test.ts`).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use serde_json::json;
use strands_agents::background_tasks::{BackgroundTasksConfig, BACKGROUND_PROPERTY, MANAGE_TOOL_NAME};
use strands_agents::hooks::{BeforeToolCallEvent, Cancel};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::{function_tool, ToolRef};
use strands_agents::types::{ContentBlock, Role, ToolResultContent, ToolResultStatus, ToolUseBlock};
use strands_agents::{Agent, Message};

fn slow_tool() -> ToolRef {
    Arc::new(
        function_tool("slow")
            .description("Sleeps, then answers")
            .input_schema(json!({ "type": "object", "properties": { "city": { "type": "string" } } }))
            .handler(|_input, _ctx| async move {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok(json!({ "forecast": "sunny" }))
            }),
    )
}

fn tool_use(name: &str, id: &str, input: serde_json::Value) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse(ToolUseBlock::new(name, id, input))],
    )
}

#[tokio::test]
async fn dispatches_a_flagged_tool_call_and_delivers_the_result_on_the_next_invoke() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(tool_use(
            "slow",
            "call-1",
            json!({ "city": "Berlin", BACKGROUND_PROPERTY: true }),
        )),
        MockResponse::message(Message::assistant("dispatched, carrying on")),
        MockResponse::message(Message::assistant("the forecast is sunny")),
    ]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_ref(slow_tool())
        .background_tasks(BackgroundTasksConfig::new().wait_for_completion(false))
        .build()
        .await
        .unwrap();

    let first = agent.invoke("forecast for Berlin?").await.unwrap();
    assert_eq!(first.to_string(), "dispatched, carrying on");

    let dispatch = agent
        .messages()
        .into_iter()
        .find_map(|message| {
            message.content.into_iter().find_map(|block| match block {
                ContentBlock::ToolResult(result) if result.tool_use_id == "call-1" => Some(result),
                _ => None,
            })
        })
        .expect("a tool result for the dispatched call");
    assert_eq!(dispatch.status, ToolResultStatus::Success);
    assert!(
        dispatch
            .text()
            .starts_with("Background task dispatched.\n\nTask ID: "),
        "{}",
        dispatch.text()
    );
    assert!(
        dispatch
            .text()
            .ends_with("The final result will be delivered automatically when the task completes."),
        "{}",
        dispatch.text()
    );

    // The real tool never sees the selection flag.
    let tool_input = agent
        .messages()
        .into_iter()
        .find_map(|message| {
            message.content.into_iter().find_map(|block| match block {
                ContentBlock::ToolUse(tool_use) if tool_use.tool_use_id == "call-1" => Some(tool_use.input),
                _ => None,
            })
        })
        .unwrap();
    assert_eq!(tool_input, json!({ "city": "Berlin", BACKGROUND_PROPERTY: true }));

    tokio::time::sleep(Duration::from_millis(150)).await;
    let before = agent.messages().len();
    let second = agent.invoke("anything back?").await.unwrap();
    assert_eq!(second.to_string(), "the forecast is sunny");

    let delivered = &agent.messages()[before..];
    let ContentBlock::ToolUse(synthetic) = &delivered[0].content[0] else {
        panic!("expected the synthetic get call, got {:?}", delivered[0]);
    };
    assert_eq!(delivered[0].role, Role::Assistant);
    assert_eq!(synthetic.name, MANAGE_TOOL_NAME);
    assert_eq!(synthetic.input["mode"], json!("get"));

    let ContentBlock::ToolResult(result) = &delivered[1].content[0] else {
        panic!("expected the synthetic tool result");
    };
    assert_eq!(delivered[1].role, Role::User);
    assert_eq!(result.status, ToolResultStatus::Success);
    assert_eq!(result.tool_use_id, synthetic.tool_use_id);
    let ToolResultContent::Json(metadata) = &result.content[0] else {
        panic!("expected task metadata");
    };
    assert_eq!(metadata["status"], json!("completed"));
    assert_eq!(metadata["toolName"], json!("slow"));
    assert_eq!(
        result.content[1],
        ToolResultContent::Json(json!({ "forecast": "sunny" }))
    );
}

#[tokio::test]
async fn offers_the_background_flag_only_for_agentic_tools() {
    let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("hi"))]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_ref(slow_tool())
        .background_tasks(BackgroundTasksConfig::new())
        .build()
        .await
        .unwrap();
    agent.invoke("hi").await.unwrap();

    let (_, options) = model.calls().pop().unwrap();
    let spec = options
        .tool_specs
        .iter()
        .find(|spec| spec.name == "slow")
        .expect("the slow tool spec");
    assert_eq!(
        spec.input_schema.as_ref().unwrap()["properties"][BACKGROUND_PROPERTY]["type"],
        json!("boolean")
    );
    let manage = options
        .tool_specs
        .iter()
        .find(|spec| spec.name == MANAGE_TOOL_NAME)
        .expect("the manage tool spec");
    assert!(manage.input_schema.as_ref().unwrap()["properties"]
        .get(BACKGROUND_PROPERTY)
        .is_none());
}

#[tokio::test]
async fn waits_for_outstanding_tasks_before_the_invocation_finishes() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(tool_use("slow", "call-1", json!({ BACKGROUND_PROPERTY: true }))),
        MockResponse::message(Message::assistant("dispatched")),
        MockResponse::message(Message::assistant("done with the result")),
    ]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_ref(slow_tool())
        .background_tasks(true)
        .build()
        .await
        .unwrap();

    let result = agent.invoke("go").await.unwrap();

    assert_eq!(result.to_string(), "done with the result");
    assert!(agent.messages().iter().any(|message| message
        .content
        .iter()
        .any(|block| matches!(block, ContentBlock::ToolUse(use_) if use_.name == MANAGE_TOOL_NAME))));
}

#[tokio::test]
async fn a_denying_before_tool_call_hook_beats_background_routing() {
    let ran = Arc::new(AtomicBool::new(false));
    let flag = Arc::clone(&ran);
    let tool: ToolRef = Arc::new(
        function_tool("slow")
            .description("Sleeps, then answers")
            .input_schema(json!({ "type": "object", "properties": { "city": { "type": "string" } } }))
            .handler(move |_input, _ctx| {
                let flag = Arc::clone(&flag);
                async move {
                    flag.store(true, Ordering::SeqCst);
                    Ok(json!({ "forecast": "sunny" }))
                }
            }),
    );
    let model = MockMessageModel::shared(vec![
        MockResponse::message(tool_use(
            "slow",
            "call-1",
            json!({ "city": "Berlin", BACKGROUND_PROPERTY: true }),
        )),
        MockResponse::message(Message::assistant("denied, carrying on")),
    ]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_ref(tool)
        .background_tasks(BackgroundTasksConfig::new())
        .build()
        .await
        .unwrap();
    let _ = agent
        .hooks()
        .add_sync_callback(|event: &mut BeforeToolCallEvent| {
            event.cancel = Cancel::Reason("denied by policy".to_owned());
            Ok(())
        });

    agent.invoke("forecast for Berlin?").await.unwrap();

    let result = agent
        .messages()
        .into_iter()
        .find_map(|message| {
            message.content.into_iter().find_map(|block| match block {
                ContentBlock::ToolResult(result) if result.tool_use_id == "call-1" => Some(result),
                _ => None,
            })
        })
        .expect("a tool result for the denied call");
    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(result.text(), "denied by policy");
    assert!(
        !ran.load(Ordering::SeqCst),
        "the tool ran despite the denying hook"
    );
    let tasks = agent.background_tasks().expect("background tasks").list();
    assert!(tasks.is_empty(), "a task was dispatched anyway: {tasks:?}");
}

/// Two flagged calls in one pass with `max_concurrency: 1` must not overlap.
#[tokio::test]
async fn max_concurrency_one_runs_background_calls_sequentially() {
    let live = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let peak = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let tool: ToolRef = {
        let live = Arc::clone(&live);
        let peak = Arc::clone(&peak);
        Arc::new(
            function_tool("slow")
                .description("Sleeps, then answers")
                .input_schema(json!({ "type": "object", "properties": { "city": { "type": "string" } } }))
                .handler(move |_input, _ctx| {
                    let live = Arc::clone(&live);
                    let peak = Arc::clone(&peak);
                    async move {
                        let now = live.fetch_add(1, Ordering::SeqCst) + 1;
                        peak.fetch_max(now, Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(40)).await;
                        live.fetch_sub(1, Ordering::SeqCst);
                        Ok(json!({ "forecast": "sunny" }))
                    }
                }),
        )
    };
    let two_calls = Message::new(
        Role::Assistant,
        vec![
            ContentBlock::ToolUse(ToolUseBlock::new(
                "slow",
                "call-1",
                json!({ "city": "Berlin", BACKGROUND_PROPERTY: true }),
            )),
            ContentBlock::ToolUse(ToolUseBlock::new(
                "slow",
                "call-2",
                json!({ "city": "Paris", BACKGROUND_PROPERTY: true }),
            )),
        ],
    );
    let model = MockMessageModel::shared(vec![
        MockResponse::message(two_calls),
        MockResponse::message(Message::assistant("both dispatched")),
    ]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_ref(tool)
        .background_tasks(
            BackgroundTasksConfig::new()
                .wait_for_completion(false)
                .max_concurrency(1),
        )
        .build()
        .await
        .unwrap();

    agent.invoke("two forecasts please").await.unwrap();
    let background = agent.background_tasks().expect("background tasks");
    assert_eq!(background.list().len(), 2);
    for _ in 0..100 {
        if background.list().iter().all(|task| task.status.is_terminal()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let statuses: Vec<_> = background.list().iter().map(|task| task.status).collect();
    assert!(
        background.list().iter().all(|task| task.status.is_terminal()),
        "tasks never settled: {statuses:?}"
    );
    assert_eq!(peak.load(Ordering::SeqCst), 1, "the two tasks overlapped");
}

/// A task that outlives the configured timeout is delivered as a failed task.
#[tokio::test]
async fn a_timed_out_background_task_is_delivered_as_failed() {
    let model = MockMessageModel::shared(vec![
        MockResponse::message(tool_use(
            "slow",
            "call-1",
            json!({ "city": "Berlin", BACKGROUND_PROPERTY: true }),
        )),
        MockResponse::message(Message::assistant("dispatched, carrying on")),
        MockResponse::message(Message::assistant("it timed out")),
    ]);
    let agent = Agent::builder()
        .model_arc(Arc::clone(&model) as Arc<dyn strands_agents::models::Model>)
        .tool_ref(slow_tool())
        .background_tasks(
            BackgroundTasksConfig::new()
                .wait_for_completion(false)
                .timeout(Duration::from_millis(10)),
        )
        .build()
        .await
        .unwrap();

    agent.invoke("forecast for Berlin?").await.unwrap();
    tokio::time::sleep(Duration::from_millis(80)).await;
    let before = agent.messages().len();
    agent.invoke("anything back?").await.unwrap();

    let delivered = &agent.messages()[before..];
    let ContentBlock::ToolResult(result) = &delivered[1].content[0] else {
        panic!("expected the synthetic tool result, got {:?}", delivered[1]);
    };
    let ToolResultContent::Json(metadata) = &result.content[0] else {
        panic!("expected task metadata");
    };
    assert_eq!(metadata["status"], json!("failed"));
    assert_eq!(
        metadata["error"],
        json!({ "type": "timeout", "message": "Timed out after 10ms" })
    );
    assert_eq!(result.content.len(), 1, "a timed-out task carries no tool result");
}
