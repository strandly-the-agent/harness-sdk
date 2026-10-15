//! End-to-end A2A test: an in-process [`A2AServer`] on an ephemeral port, driven by [`A2AAgent`]
//! and by a local agent holding the `a2a_client` tool. No HTTP mocking — both halves are real.

#![cfg(feature = "a2a")]

use std::sync::Arc;

use futures::StreamExt;
use serde_json::json;
use strands_agents::a2a::types::{AgentSkill, JsonRpcError, Task, TaskState, PROTOCOL_VERSION};
use strands_agents::a2a::{A2AAgent, A2AExecutor, A2AServer, A2AStreamEvent, ServerHandle};
use strands_agents::models::Model;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::vended::A2AClientTool;
use strands_agents::types::{ContentBlock, Message, Role, ToolUseBlock};
use strands_agents::{Agent, StopReason};

/// An agent that answers with `replies`, in order.
fn agent_saying(replies: &[&str]) -> Agent {
    let responses = replies
        .iter()
        .map(|text| MockResponse::message(Message::new(Role::Assistant, vec![ContentBlock::text(*text)])))
        .collect();
    Agent::builder()
        .model(MockMessageModel::shared(responses) as Arc<dyn Model>)
        .build()
        .unwrap()
}

/// A served agent on `127.0.0.1:0`, one fresh agent per A2A context.
async fn serve(replies: &'static [&'static str]) -> ServerHandle {
    A2AServer::builder("E2E Agent")
        .description("Answers questions end to end")
        .version("1.2.3")
        .skills([AgentSkill::new(
            "answer",
            "Answer",
            "Answers a question",
            ["qa".to_owned()],
        )])
        .executor(A2AExecutor::with_factory(move |_| agent_saying(replies)))
        .build()
        .unwrap()
        .serve()
        .await
        .unwrap()
}

fn base_url(handle: &ServerHandle) -> String {
    format!("http://{}", handle.local_addr())
}

async fn rpc(base: &str, method: &str, params: serde_json::Value) -> serde_json::Value {
    reqwest::Client::new()
        .post(base)
        .json(&json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn a2a_agent_invokes_the_served_agent() {
    let handle = serve(&["Hello from the server"]).await;
    let remote = A2AAgent::builder(base_url(&handle)).build();

    let result = remote.invoke("hi").await.unwrap();

    assert_eq!(result.text(), "Hello from the server");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    handle.shutdown().await;
}

#[tokio::test]
async fn a2a_agent_discovers_the_served_agent_card() {
    let handle = serve(&["Hello"]).await;
    let remote = A2AAgent::builder(base_url(&handle)).build();

    let card = remote.agent_card().await.unwrap().clone();

    assert_eq!(card.name, "E2E Agent");
    assert_eq!(card.description, "Answers questions end to end");
    assert_eq!(card.version, "1.2.3");
    assert_eq!(card.url, base_url(&handle));
    assert_eq!(card.protocol_version, PROTOCOL_VERSION);
    assert_eq!(card.capabilities.streaming, Some(true));
    assert_eq!(card.skills[0].id, "answer");
    assert_eq!(remote.name(), "E2E Agent");
    assert_eq!(remote.description(), Some("Answers questions end to end"));
    handle.shutdown().await;
}

#[tokio::test]
async fn a2a_agent_streams_protocol_events_then_a_result() {
    let handle = serve(&["Streamed"]).await;
    let remote = A2AAgent::builder(base_url(&handle)).build();

    let events: Vec<A2AStreamEvent> = Box::pin(remote.stream("hi"))
        .map(Result::unwrap)
        .collect::<Vec<_>>()
        .await;

    let kinds: Vec<&str> = events
        .iter()
        .filter_map(|e| e.event().map(|e| e.kind()))
        .collect();
    assert!(!kinds.is_empty(), "expected protocol events, got {events:?}");
    assert_eq!(events.last().unwrap().type_name(), "a2aResultEvent");
    assert_eq!(events.last().unwrap().result().unwrap().text(), "Streamed");
    handle.shutdown().await;
}

#[tokio::test]
async fn a2a_agent_keeps_one_remote_context_across_turns_only_when_asked() {
    let handle = serve(&["first", "second"]).await;
    let remote = A2AAgent::builder(base_url(&handle))
        .continue_context(true)
        .build();

    assert_eq!(remote.invoke("one").await.unwrap().text(), "first");
    let context = remote.context_id().expect("the server assigned a context");
    assert_eq!(remote.invoke("two").await.unwrap().text(), "second");
    assert_eq!(remote.context_id().as_deref(), Some(context.as_str()));

    let fresh = A2AAgent::builder(base_url(&handle)).build();
    fresh.invoke("one").await.unwrap();
    assert_eq!(fresh.context_id(), None, "continuity is opt-in");
    handle.shutdown().await;
}

#[tokio::test]
async fn a2a_agent_reports_an_unreachable_server_as_a_model_error() {
    let remote = A2AAgent::builder("http://127.0.0.1:1")
        .timeout(std::time::Duration::from_millis(500))
        .build();

    let error = remote.invoke("hi").await.unwrap_err();

    assert!(error.is_model_error(), "{error:?}");
}

#[tokio::test]
async fn a_local_agent_discovers_then_messages_the_served_agent_with_the_a2a_client_tool() {
    let handle = serve(&["Remote says hi"]).await;
    let endpoint = base_url(&handle);
    let tool = A2AClientTool::new([endpoint.clone()]).unwrap();

    let model = MockMessageModel::shared(vec![
        MockResponse::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "a2a_client",
                "call-1",
                json!({ "operation": "discover", "endpoint": endpoint }),
            ))],
        )),
        MockResponse::message(Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock::new(
                "a2a_client",
                "call-2",
                json!({ "operation": "send_message", "endpoint": endpoint, "message": "hello" }),
            ))],
        )),
        MockResponse::message(Message::assistant("done")),
    ]);
    let local = Agent::builder()
        .model(model as Arc<dyn Model>)
        .tools([Arc::new(tool) as strands_agents::tools::ToolRef])
        .build()
        .unwrap();

    let result = local.invoke("talk to the remote agent").await.unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    let results: Vec<String> = local
        .messages()
        .iter()
        .flat_map(|message| message.tool_results())
        .map(|tool_result| serde_json::to_string(&tool_result.content).unwrap())
        .collect();
    assert_eq!(results.len(), 2, "{results:?}");
    assert!(results[0].contains("E2E Agent"), "discovery: {}", results[0]);
    assert!(results[1].contains("Remote says hi"), "send: {}", results[1]);
    handle.shutdown().await;
}

#[tokio::test]
async fn tasks_get_returns_the_task_a2a_agent_created_and_cancel_rejects_it() {
    let handle = serve(&["Hello"]).await;
    let base = base_url(&handle);
    let remote = A2AAgent::builder(&base).build();

    let events: Vec<A2AStreamEvent> = Box::pin(remote.stream("hi"))
        .map(Result::unwrap)
        .collect::<Vec<_>>()
        .await;
    let task_id = events
        .iter()
        .filter_map(A2AStreamEvent::event)
        .find_map(|event| match event {
            strands_agents::a2a::types::A2AEventData::Task(task) => Some(task.id.clone()),
            strands_agents::a2a::types::A2AEventData::StatusUpdate(update) => Some(update.task_id.clone()),
            strands_agents::a2a::types::A2AEventData::ArtifactUpdate(update) => Some(update.task_id.clone()),
            _ => None,
        })
        .expect("the server created a task");

    let fetched = rpc(&base, "tasks/get", json!({ "id": task_id })).await;
    let task: Task = serde_json::from_value(fetched["result"].clone()).unwrap();
    assert_eq!(task.id, task_id);
    assert_eq!(task.status.state, TaskState::Completed);

    let cancelled = rpc(&base, "tasks/cancel", json!({ "id": task_id })).await;
    let error: JsonRpcError = serde_json::from_value(cancelled["error"].clone()).unwrap();
    assert_eq!(
        error.code,
        strands_agents::a2a::types::error_codes::TASK_NOT_CANCELABLE
    );

    let missing = rpc(&base, "tasks/get", json!({ "id": "no-such-task" })).await;
    assert_eq!(
        missing["error"]["code"],
        strands_agents::a2a::types::error_codes::TASK_NOT_FOUND
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn serves_concurrent_requests_in_isolated_contexts() {
    let handle = serve(&["Hello"]).await;
    let base = base_url(&handle);

    let invocations = (0..5).map(|i| {
        let agent = A2AAgent::builder(&base).build();
        async move { (i, agent.invoke(format!("question {i}")).await.unwrap()) }
    });
    let results = futures::future::join_all(invocations).await;

    assert_eq!(results.len(), 5);
    for (i, result) in results {
        assert_eq!(result.text(), "Hello", "invocation {i}");
    }
    handle.shutdown().await;
}
