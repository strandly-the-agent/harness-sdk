//! The worker protocol end to end over in-memory pipes, with a mock model.

use std::sync::Arc;

use serde_json::{json, Value as JsonValue};
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::function_tool;
use strands_agents::types::{ContentBlock, Message, Role, StopReason, ToolUseBlock, Usage};
use strands_agents::Agent;
use strands_cli_worker::{factory, run, BuildOptions, BuiltAgent};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};

struct Worker {
    stdin: DuplexStream,
    messages: tokio::io::Lines<BufReader<DuplexStream>>,
    task: tokio::task::JoinHandle<Result<(), String>>,
}

impl Worker {
    fn spawn(script: impl Fn() -> Vec<MockResponse> + Send + Sync + 'static) -> Self {
        let script = Arc::new(script);
        let make = factory(move |options: BuildOptions| {
            let script = script.clone();
            async move {
                // Every (re)build replays the same scripted responses from the start.
                let responses = script();
                let mut builder = Agent::builder()
                    .name(
                        options
                            .overrides
                            .get("name")
                            .and_then(JsonValue::as_str)
                            .unwrap_or("Mock"),
                    )
                    .model(MockMessageModel::new(responses))
                    .tool(
                        function_tool("echo")
                            .description("Echo the input")
                            .handler(|input: JsonValue, _| async move { Ok(input) }),
                    )
                    .tool(
                        function_tool("confirm")
                            .description("Asks before acting")
                            .handler(
                                |_: JsonValue, ctx: strands_agents::tools::ToolContext| async move {
                                    let approved: bool = ctx.interrupt(
                                        strands_agents::types::InterruptParams::new("confirm")
                                            .with_reason(json!({ "why": "destructive" })),
                                    )?;
                                    Ok(if approved { "confirmed" } else { "declined" })
                                },
                            ),
                    );
                if let Some(description) = options.overrides.get("description").and_then(JsonValue::as_str) {
                    builder = builder.description(description);
                }
                Ok(BuiltAgent {
                    agent: builder.build()?,
                    skills: None,
                    session_dir: None,
                })
            }
        });
        let (stdin, worker_stdin) = tokio::io::duplex(64 * 1024);
        let (worker_out, out) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(run(
            BufReader::new(worker_stdin),
            worker_out,
            make,
            std::env::temp_dir(),
        ));
        Self {
            stdin,
            messages: BufReader::new(out).lines(),
            task,
        }
    }

    async fn send(&mut self, command: JsonValue) {
        let mut line = command.to_string();
        line.push('\n');
        self.stdin.write_all(line.as_bytes()).await.unwrap();
    }

    async fn next(&mut self) -> JsonValue {
        let line = self.messages.next_line().await.unwrap().expect("worker closed");
        if std::env::var_os("WORKER_TRACE").is_some() {
            eprintln!("<< {line}");
        }
        serde_json::from_str(&line).unwrap()
    }

    /// Drain until a terminal message (`ready` / `result` / `error`), returning the events seen.
    async fn until_terminal(&mut self) -> (Vec<JsonValue>, JsonValue) {
        let mut events = Vec::new();
        loop {
            let message = self.next().await;
            match message["type"].as_str() {
                Some("event") => events.push(message["event"].clone()),
                Some("ready" | "result" | "error") => return (events, message),
                other => panic!("unexpected message type {other:?}: {message}"),
            }
        }
    }

    async fn init(&mut self, extra: JsonValue) -> JsonValue {
        let mut command = json!({ "type": "init", "overrides": { "session": false } });
        if let (Some(target), Some(source)) = (command.as_object_mut(), extra.as_object()) {
            target.extend(source.clone());
        }
        self.send(command).await;
        let (events, ready) = self.until_terminal().await;
        assert!(events.is_empty());
        assert_eq!(ready["type"], "ready", "{ready}");
        ready
    }

    async fn close(mut self) {
        self.send(json!({ "type": "close" })).await;
        self.stdin.shutdown().await.unwrap();
        self.task.await.unwrap().unwrap();
    }
}

fn text(text: &str) -> MockResponse {
    MockResponse::with_usage(
        Message::assistant(text),
        Usage {
            input_tokens: 10,
            output_tokens: 5,
            total_tokens: 15,
            cache_read_input_tokens: None,
            cache_write_input_tokens: None,
        },
    )
}

fn tool_call(name: &str, id: &str, input: JsonValue) -> MockResponse {
    MockResponse::with_stop_reason(
        Message::new(
            Role::Assistant,
            vec![ContentBlock::ToolUse(ToolUseBlock {
                name: name.to_owned(),
                tool_use_id: id.to_owned(),
                input,
                reasoning_signature: None,
            })],
        ),
        StopReason::ToolUse,
    )
}

#[tokio::test]
async fn init_reports_state_and_a_prompt_streams_a_turn() {
    let mut worker = Worker::spawn(|| vec![text("hello there")]);
    let ready = worker
        .init(json!({ "overrides": { "session": false, "name": "Tester", "model": "bedrock/x" } }))
        .await;
    let state = &ready["state"];
    assert_eq!(state["name"], "Tester");
    assert_eq!(state["model"], "bedrock/x");
    assert_eq!(state["modelSpecifier"], "bedrock/x");
    assert_eq!(state["thinking"], "auto");
    assert_eq!(state["reconstructable"], true);
    assert!(state.get("session").is_none());
    assert_eq!(state["messages"], json!([]));
    assert!(state["tools"]
        .as_array()
        .unwrap()
        .iter()
        .any(|t| t["name"] == "echo"));

    worker.send(json!({ "type": "prompt", "prompt": "hi" })).await;
    let (events, result) = worker.until_terminal().await;
    let streamed: String = events
        .iter()
        .filter(|e| e["type"] == "textDelta")
        .map(|e| e["text"].as_str().unwrap())
        .collect();
    assert_eq!(streamed, "hello there");
    assert_eq!(result["type"], "result", "{result}");
    assert_eq!(result["result"]["stopReason"], "endTurn");
    assert_eq!(result["result"]["finalText"], "hello there");
    assert_eq!(result["result"]["usage"]["totalTokens"], 15);
    assert_eq!(result["result"]["usage"]["cacheReadInputTokens"], 0);
    assert_eq!(result["reload"], false);
    let messages = result["state"]["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 2);
    assert_eq!(
        messages[0],
        json!({ "role": "user", "content": [{ "text": "hi" }] })
    );
    worker.close().await;
}

#[tokio::test]
async fn interactive_tool_calls_are_authorized_and_denials_cancel_the_tool() {
    let mut worker = Worker::spawn(|| vec![tool_call("echo", "t1", json!({ "x": 1 })), text("done")]);
    worker.init(json!({ "interactive": true })).await;
    worker.send(json!({ "type": "prompt", "prompt": "go" })).await;

    let mut authorize = None;
    let mut events = Vec::new();
    while authorize.is_none() {
        let message = worker.next().await;
        match message["type"].as_str() {
            Some("authorize") => authorize = Some(message),
            Some("event") => events.push(message["event"].clone()),
            other => panic!("unexpected {other:?}: {message}"),
        }
    }
    let authorize = authorize.unwrap();
    assert_eq!(authorize["name"], "echo");
    assert_eq!(authorize["input"], json!({ "x": 1 }));
    assert_eq!(authorize["pathInWorkspace"], false);
    assert!(events
        .iter()
        .any(|e| e["type"] == "toolStart" && e["toolUseId"] == "t1"));

    worker
        .send(json!({ "type": "permission", "requestId": authorize["id"], "approved": false }))
        .await;
    let (events, result) = worker.until_terminal().await;
    let tool_result = events
        .iter()
        .find(|e| e["type"] == "toolResult")
        .expect("toolResult event");
    assert_eq!(tool_result["status"], "error");
    assert!(tool_result["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("DENIED"));
    assert_eq!(result["result"]["stopReason"], "endTurn");
    worker.close().await;
}

#[tokio::test]
async fn approved_tool_calls_run() {
    let mut worker = Worker::spawn(|| vec![tool_call("echo", "t1", json!({ "x": 1 })), text("done")]);
    worker.init(json!({ "interactive": true })).await;
    worker.send(json!({ "type": "prompt", "prompt": "go" })).await;
    let mut message = worker.next().await;
    while message["type"] != "authorize" {
        message = worker.next().await;
    }
    worker
        .send(json!({ "type": "permission", "requestId": message["id"], "approved": true }))
        .await;
    let (events, result) = worker.until_terminal().await;
    let tool_result = events.iter().find(|e| e["type"] == "toolResult").unwrap();
    assert_eq!(tool_result["status"], "success");
    assert_eq!(
        tool_result["content"],
        json!([{ "type": "json", "value": { "x": 1 } }])
    );
    assert_eq!(result["result"]["finalText"], "done");
    worker.close().await;
}

#[tokio::test]
async fn cancel_while_waiting_for_authorization_ends_the_turn_as_cancelled() {
    let mut worker = Worker::spawn(|| vec![tool_call("echo", "t1", json!({})), text("never")]);
    worker.init(json!({ "interactive": true })).await;
    worker.send(json!({ "type": "prompt", "prompt": "go" })).await;
    let mut message = worker.next().await;
    while message["type"] != "authorize" {
        message = worker.next().await;
    }
    worker.send(json!({ "type": "cancel" })).await;
    let (_, result) = worker.until_terminal().await;
    assert_eq!(result["type"], "result", "{result}");
    assert_eq!(result["result"], json!({ "stopReason": "cancelled" }));
    worker.close().await;
}

#[tokio::test]
async fn commands_while_busy_are_rejected_and_permission_is_always_accepted() {
    let mut worker = Worker::spawn(|| vec![tool_call("echo", "t1", json!({})), text("done")]);
    worker.init(json!({ "interactive": true })).await;
    worker.send(json!({ "type": "prompt", "prompt": "go" })).await;
    let mut message = worker.next().await;
    while message["type"] != "authorize" {
        message = worker.next().await;
    }
    worker.send(json!({ "type": "compact" })).await;
    let busy = worker.next().await;
    assert_eq!(busy["type"], "error");
    assert!(busy["message"].as_str().unwrap().contains("busy"));
    worker
        .send(json!({ "type": "permission", "requestId": message["id"], "approved": true }))
        .await;
    let (_, result) = worker.until_terminal().await;
    assert_eq!(result["type"], "result");
    worker.close().await;
}

#[tokio::test]
async fn reset_carries_the_conversation_unless_cleared() {
    let mut worker = Worker::spawn(|| vec![text("one")]);
    worker.init(json!({})).await;
    worker.send(json!({ "type": "prompt", "prompt": "a" })).await;
    let (_, result) = worker.until_terminal().await;
    assert_eq!(result["state"]["messages"].as_array().unwrap().len(), 2);

    worker
        .send(json!({ "type": "reset", "model": "bedrock/other", "thinking": null }))
        .await;
    let (_, reset) = worker.until_terminal().await;
    assert_eq!(reset["result"]["stopReason"], "reset");
    assert_eq!(reset["state"]["model"], "bedrock/other");
    assert_eq!(reset["state"]["thinking"], JsonValue::Null);
    assert_eq!(reset["state"]["messages"].as_array().unwrap().len(), 2);

    worker
        .send(json!({ "type": "reset", "clear": true, "sessionId": "fresh" }))
        .await;
    let (_, cleared) = worker.until_terminal().await;
    assert_eq!(cleared["state"]["messages"], json!([]));
    assert_eq!(cleared["state"]["model"], "bedrock/other");
    worker.close().await;
}

#[tokio::test]
async fn conversation_round_trips_and_snapshot_seeds_a_fork() {
    let mut worker = Worker::spawn(|| vec![text("remembered")]);
    worker.init(json!({})).await;
    worker.send(json!({ "type": "prompt", "prompt": "a" })).await;
    worker.until_terminal().await;

    worker.send(json!({ "type": "conversation" })).await;
    let (_, captured) = worker.until_terminal().await;
    assert_eq!(captured["result"]["stopReason"], "snapshot");
    let snapshot: JsonValue =
        serde_json::from_str(captured["result"]["finalText"].as_str().unwrap()).unwrap();
    assert_eq!(snapshot["scope"], "agent");
    assert!(snapshot.get("schemaVersion").is_some());
    assert_eq!(snapshot["data"]["messages"].as_array().unwrap().len(), 2);

    worker.send(json!({ "type": "snapshot" })).await;
    let (_, fork_source) = worker.until_terminal().await;
    let serialized = fork_source["result"]["finalText"].as_str().unwrap().to_owned();

    let mut target = Worker::spawn(|| vec![text("x")]);
    target.init(json!({})).await;
    target
        .send(json!({ "type": "seed", "snapshot": serialized, "stash": "{}" }))
        .await;
    let (_, seeded) = target.until_terminal().await;
    assert_eq!(seeded["result"]["stopReason"], "seeded");
    assert_eq!(seeded["state"]["messages"], captured["state"]["messages"]);

    let mut restored = Worker::spawn(|| vec![text("x")]);
    restored.init(json!({})).await;
    restored
        .send(json!({ "type": "conversation", "snapshot": snapshot, "preserveReasoning": false }))
        .await;
    let (_, result) = restored.until_terminal().await;
    assert_eq!(result["result"]["stopReason"], "restored");
    assert_eq!(result["state"]["messages"], captured["state"]["messages"]);

    worker.close().await;
    target.close().await;
    restored.close().await;
}

#[tokio::test]
async fn reload_failed_is_recorded_and_unknown_skills_error() {
    let mut worker = Worker::spawn(|| vec![text("x")]);
    worker.init(json!({})).await;
    worker
        .send(json!({ "type": "reload-failed", "message": "bad model" }))
        .await;
    let (_, recorded) = worker.until_terminal().await;
    assert_eq!(recorded["result"]["stopReason"], "recorded");
    assert!(recorded["state"]["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .contains("bad model"));
    worker.send(json!({ "type": "skill", "name": "nope" })).await;
    let (_, error) = worker.until_terminal().await;
    assert_eq!(error["type"], "error");
    worker.close().await;
}

#[tokio::test]
async fn init_failure_is_reported_and_exits() {
    let make = factory(|_: BuildOptions| async { Err(strands_agents::Error::other("boom")) });
    let (mut stdin, worker_stdin) = tokio::io::duplex(1024);
    let (worker_out, out) = tokio::io::duplex(1024);
    let task = tokio::spawn(run(
        BufReader::new(worker_stdin),
        worker_out,
        make,
        std::env::temp_dir(),
    ));
    stdin.write_all(b"{\"type\":\"init\"}\n").await.unwrap();
    let mut lines = BufReader::new(out).lines();
    let line = lines.next_line().await.unwrap().unwrap();
    let message: JsonValue = serde_json::from_str(&line).unwrap();
    assert_eq!(message["type"], "error");
    assert!(message["message"].as_str().unwrap().contains("boom"));
    assert!(task.await.unwrap().is_err());
}

#[tokio::test]
async fn sdk_interrupts_become_permission_events_and_resume_with_the_answer() {
    let mut worker = Worker::spawn(|| vec![tool_call("confirm", "c1", json!({})), text("after")]);
    worker.init(json!({})).await;
    worker.send(json!({ "type": "prompt", "prompt": "go" })).await;
    let mut message = worker.next().await;
    while !(message["type"] == "event" && message["event"]["type"] == "permission") {
        message = worker.next().await;
    }
    let request = &message["event"]["request"];
    assert_eq!(request["toolName"], "confirm");
    assert_eq!(request["input"], json!({ "why": "destructive" }));
    assert_eq!(request["options"][0]["id"], "allow");
    worker
        .send(json!({ "type": "permission", "requestId": request["id"], "approved": true }))
        .await;
    let (events, result) = worker.until_terminal().await;
    let tool_result = events
        .iter()
        .find(|e| e["type"] == "toolResult")
        .expect("toolResult after resume");
    assert_eq!(tool_result["content"][0]["text"], "confirmed");
    assert_eq!(result["result"]["stopReason"], "endTurn");
    assert_eq!(result["result"]["finalText"], "after");
    worker.close().await;
}

#[tokio::test]
async fn a_cancelled_turn_leaves_no_stale_authorize_for_the_next_one() {
    // The mock replays one response per model call: turn 1 is cancelled after the first.
    let mut worker = Worker::spawn(|| {
        vec![
            tool_call("echo", "t1", json!({})),
            tool_call("echo", "t2", json!({})),
            text("done"),
        ]
    });
    worker.init(json!({ "interactive": true })).await;
    worker.send(json!({ "type": "prompt", "prompt": "go" })).await;
    let mut message = worker.next().await;
    while message["type"] != "authorize" {
        message = worker.next().await;
    }
    worker.send(json!({ "type": "cancel" })).await;
    let (_, cancelled) = worker.until_terminal().await;
    assert_eq!(cancelled["result"]["stopReason"], "cancelled");

    worker.send(json!({ "type": "prompt", "prompt": "again" })).await;
    let mut authorizes = 0;
    loop {
        let message = worker.next().await;
        match message["type"].as_str() {
            Some("authorize") => {
                authorizes += 1;
                worker
                    .send(json!({ "type": "permission", "requestId": message["id"], "approved": true }))
                    .await;
            }
            Some("result") => break,
            Some("error") => panic!("{message}"),
            _ => {}
        }
    }
    assert_eq!(
        authorizes, 1,
        "exactly one authorize for the new turn's single tool call"
    );
    worker.close().await;
}

#[tokio::test]
async fn close_during_a_turn_exits_promptly() {
    let mut worker = Worker::spawn(|| vec![tool_call("echo", "t1", json!({})), text("never")]);
    worker.init(json!({ "interactive": true })).await;
    worker.send(json!({ "type": "prompt", "prompt": "go" })).await;
    let mut message = worker.next().await;
    while message["type"] != "authorize" {
        message = worker.next().await;
    }
    let started = std::time::Instant::now();
    worker.close().await;
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
}
