//! Follow-up input, ported from strands-ts/src/agent/__tests__/agent.continuation.test.ts. The
//! TS tests reach the internal `continuations.addInput`; these use the crate-internal event
//! methods.

use super::*;
use crate::agent::continuation::ContinuationInput;
use crate::interrupt::{InterruptParams, InterruptResponse};
use crate::test_support::stream_tool;
use crate::tools::{ToolOutcome, ToolStream};
use crate::types::json::JsonObject;

type Log = Arc<Mutex<Vec<String>>>;

fn push(log: &Log, entry: &str) {
    log.lock().unwrap().push(entry.to_owned());
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn joined(message: &Message) -> String {
    texts(message).concat()
}

fn text_model(turns: &[&str]) -> Arc<MockMessageModel> {
    mock(turns.iter().map(|turn| vec![text(turn)]).collect())
}

fn quiet(model: &Arc<MockMessageModel>) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .printer(false)
        .build()
        .unwrap()
}

/// Runs `add` on the first `AfterInvocationEvent` only.
fn on_first_after(agent: &Agent, add: impl Fn(&mut AfterInvocationEvent<'_>) + Send + Sync + 'static) {
    let done = Arc::new(Mutex::new(false));
    agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
        let mut done = done.lock().unwrap();
        if !*done {
            *done = true;
            add(event);
        }
    });
}

#[tokio::test]
async fn combines_multiple_follow_up_contributions_with_public_resume_input() {
    let model = text_model(&["initial", "final"]);
    let mut agent = quiet(&model);
    let appended = Log::default();
    let abandoned = Log::default();
    let inputs = Log::default();
    let seen = Arc::clone(&inputs);
    agent.add_hook(move |event: &mut BeforeInvocationEvent<'_>| {
        let joined: Vec<String> = event.messages().iter().map(joined).collect();
        push(&seen, &joined.join("|"));
    });
    let (appended_log, abandoned_log) = (Arc::clone(&appended), Arc::clone(&abandoned));
    on_first_after(&agent, move |event| {
        for args in ["first", "second"] {
            let log = Arc::clone(&appended_log);
            event.add_continuation(ContinuationInput::new(args).on_appended(move || push(&log, args)));
        }
        let log = Arc::clone(&abandoned_log);
        let invalid = vec![Message::new(Role::Assistant, vec![text("invalid")])];
        event
            .add_continuation(ContinuationInput::new(invalid).on_abandoned(move |reason| push(&log, reason)));
        event.set_resume("public");
    });
    agent.invoke("start").await.unwrap();
    let request = &model.requests()[1].messages;
    let roles: Vec<Role> = request.iter().map(Message::role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
    assert_eq!(joined(request.last().unwrap()), "firstsecondpublic");
    let history: Vec<String> = agent.messages().iter().map(joined).collect();
    assert_eq!(history, ["start", "initial", "firstsecondpublic", "final"]);
    assert_eq!(entries(&inputs), ["start", "public"]);
    assert_eq!(entries(&appended), ["first", "second"]);
    assert_eq!(
        entries(&abandoned),
        ["Continuation input must contain a complete message sequence"]
    );
}

#[tokio::test]
async fn keeps_prepared_continuation_messages_outside_hook_mutations() {
    let model = text_model(&["initial", "final"]);
    let mut agent = quiet(&model);
    let appended = Log::default();
    let passes = Arc::new(Mutex::new(0));
    agent.add_hook(move |event: &mut BeforeInvocationEvent<'_>| {
        let mut passes = passes.lock().unwrap();
        *passes += 1;
        if *passes == 2 {
            event.set_messages(Vec::new());
        }
    });
    let log = Arc::clone(&appended);
    on_first_after(&agent, move |event| {
        let exchange = vec![
            Message::new(
                Role::Assistant,
                vec![tool_use("backgroundTask", "task-1", json!({}))],
            ),
            Message::new(
                Role::User,
                vec![ToolResultBlock::new("task-1", ToolResultStatus::Success, vec!["done".into()]).into()],
            ),
        ];
        let log = Arc::clone(&log);
        event.add_continuation(ContinuationInput::new(exchange).on_appended(move || push(&log, "appended")));
        event.set_resume("public");
    });
    agent.invoke("start").await.unwrap();
    let request = &model.requests()[1].messages;
    let shapes: Vec<JsonValue> = request
        .iter()
        .map(|message| json!({"role": message.role(), "content": message.content()}))
        .collect();
    assert_eq!(
        shapes,
        [
            json!({"role": "user", "content": [{"text": "start"}]}),
            json!({"role": "assistant", "content": [
                {"text": "initial"},
                {"toolUse": {"name": "backgroundTask", "toolUseId": "task-1", "input": {}}}
            ]}),
            json!({"role": "user", "content": [
                {"toolResult": {"toolUseId": "task-1", "status": "success", "content": [{"text": "done"}]}}
            ]}),
        ]
    );
    assert!(request.iter().all(|message| joined(message) != "public"));
    assert_eq!(entries(&appended), ["appended"]);
}

#[tokio::test]
async fn retains_follow_up_input_through_failed_resume_attempts() {
    let model = mock(vec![
        vec![tool_use("confirmTool", "tool-1", json!({}))],
        vec![text("resumed")],
        vec![text("continued")],
    ]);
    let tool = stream_tool("confirmTool", |context| {
        ToolStream::from_future(async move {
            let params = InterruptParams {
                name: "confirm".to_owned(),
                reason: Some(json!("Confirm?")),
                response: None,
            };
            match context.interrupt(params) {
                Ok(response) => ToolOutcome::Result(crate::test_support::text_result(
                    context.tool_use().tool_use_id(),
                    &format!("confirmed:{response}"),
                )),
                Err(interrupt) => ToolOutcome::Interrupt(interrupt),
            }
        })
    });
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools([tool])
        .printer(false)
        .build()
        .unwrap();
    let appended = Log::default();
    let abandoned = Log::default();
    let (appended_log, abandoned_log) = (Arc::clone(&appended), Arc::clone(&abandoned));
    on_first_after(&agent, move |event| {
        let (appended, abandoned) = (Arc::clone(&appended_log), Arc::clone(&abandoned_log));
        event.add_continuation(
            ContinuationInput::new("pending")
                .on_appended(move || push(&appended, "appended"))
                .on_abandoned(move |reason| push(&abandoned, reason)),
        );
    });
    let interrupted = agent.invoke("start").await.unwrap();
    assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
    assert!(entries(&appended).is_empty() && entries(&abandoned).is_empty());
    let error = agent.invoke("invalid resume").await.unwrap_err();
    assert!(error.to_string().contains("Agent is in an interrupted state"));
    assert!(entries(&appended).is_empty() && entries(&abandoned).is_empty());
    let id = interrupted.interrupts()[0].id().to_owned();
    let result = agent
        .invoke(vec![InterruptResponse::new(id, json!("yes"))])
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(joined(model.requests()[2].messages.last().unwrap()), "pending");
    assert_eq!(entries(&appended), ["appended"]);
    assert!(entries(&abandoned).is_empty());
}

#[tokio::test]
async fn preserves_an_unrecognized_stop_reason_instead_of_continuing() {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn_with(
        vec![text("partial")],
        MockTurnOptions::builder()
            .stop_reason("providerSpecificStop")
            .build(),
    );
    model.add_turn(vec![text("unreachable")]);
    let mut agent = quiet(&model);
    let abandoned = Log::default();
    let log = Arc::clone(&abandoned);
    on_first_after(&agent, move |event| {
        let log = Arc::clone(&log);
        event.add_continuation(
            ContinuationInput::new("pending").on_abandoned(move |reason| push(&log, reason)),
        );
    });
    let result = agent.invoke("start").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::from("providerSpecificStop"));
    assert_eq!(model.requests().len(), 1);
    let history: Vec<String> = agent.messages().iter().map(joined).collect();
    assert_eq!(history, ["start", "partial"]);
    assert_eq!(
        entries(&abandoned),
        ["Continuation abandoned after providerSpecificStop"]
    );
}

#[tokio::test]
async fn appends_a_complete_tool_exchange_contributed_before_the_model_call() {
    let model = text_model(&["final"]);
    let mut custom = JsonObject::new();
    custom.insert("pinned".to_owned(), json!(true));
    let metadata = MessageMetadata::new().with_custom(custom);
    let initial = Message::new(Role::User, vec![text("start")])
        .with_tracking_id("durable-1")
        .with_metadata(metadata.clone());
    let tool_use_message = Message::new(
        Role::Assistant,
        vec![tool_use("background-result", "delivery-1", json!({}))],
    );
    let tool_result_message = Message::new(
        Role::User,
        vec![ToolResultBlock::new("delivery-1", ToolResultStatus::Success, vec!["complete".into()]).into()],
    );
    let mut agent = quiet(&model);
    let added: Arc<Mutex<Vec<Message>>> = Arc::default();
    let added_log = Arc::clone(&added);
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
        added_log.lock().unwrap().push(event.message().clone());
    });
    let appended = Log::default();
    let (exchange, log) = (
        vec![tool_use_message.clone(), tool_result_message.clone()],
        Arc::clone(&appended),
    );
    agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        event.add_continuation(ContinuationInput::new("guidance"));
        let log = Arc::clone(&log);
        event.add_continuation(
            ContinuationInput::new(exchange.clone()).on_appended(move || push(&log, "appended")),
        );
    });
    agent.invoke(vec![initial]).await.unwrap();
    let request = &model.requests()[0].messages;
    let roles: Vec<Role> = request.iter().map(Message::role).collect();
    assert_eq!(roles, [Role::User, Role::Assistant, Role::User]);
    assert_eq!(joined(&request[0]), "startguidance");
    let history = agent.messages();
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].content(), [text("start"), text("guidance")]);
    assert_eq!(history[0].tracking_id(), "durable-1");
    assert_eq!(history[0].metadata(), Some(&metadata));
    assert_eq!(history[1].content(), tool_use_message.content());
    assert_eq!(history[2].content(), tool_result_message.content());
    let added = added.lock().unwrap();
    assert!(
        added
            .iter()
            .any(|message| message.content() == history[0].content())
    );
    assert_eq!(entries(&appended), ["appended"]);
}

#[tokio::test]
async fn abandons_follow_up_input_when_the_stream_closes_before_it_is_appended() {
    let model = text_model(&["initial", "unreachable"]);
    let mut agent = quiet(&model);
    let abandoned = Log::default();
    let log = Arc::clone(&abandoned);
    on_first_after(&agent, move |event| {
        let log = Arc::clone(&log);
        event.add_continuation(
            ContinuationInput::new("pending").on_abandoned(move |reason| push(&log, reason)),
        );
    });
    {
        let mut stream = agent.stream("start");
        while let Some(event) = stream.next().await {
            if matches!(event.unwrap(), AgentStreamEvent::MessageAdded(_)) {
                break;
            }
        }
    }
    // The dropped stream's `AfterInvocationEvent` hooks run here.
    agent.shutdown().await.unwrap();
    assert_eq!(
        entries(&abandoned),
        ["Agent stream closed before continuation input was incorporated into agent history"]
    );
    // A drop is a cancel, so history ends with the cancel message (DIVERGENCES).
    let history: Vec<String> = agent.messages().iter().map(joined).collect();
    assert_eq!(history, ["start", "Cancelled by user"]);
}
