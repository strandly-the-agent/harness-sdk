//! Ported from strands-ts/src/agent/__tests__/agent.hook.test.ts (the cases needing no later
//! subsystem).

#![expect(clippy::unwrap_used, clippy::panic, reason = "test code")]

use std::sync::{Arc, Mutex};

use futures::StreamExt;
use serde_json::{Value, json};
use strands::hooks::{
    AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, BeforeInvocationEvent, BeforeModelCallEvent,
    BeforeToolCallEvent, BeforeToolsEvent, Cancel, EndTurn, MessageAddedEvent, ModelMessageEvent,
};
use strands::testing::{HookRecorder, MockModel, test_agent};
use strands::tools::FunctionTool;
use strands::tools::executors::ToolExecutorStrategy;
use strands::types::messages::{Role, TextBlock, ToolResultBlock, ToolUseBlock};
use strands::types::tools::ToolResultStatus;
use strands::{Agent, CancelOptions, ContentBlock, Message, StopReason, Tool, ToolError};

fn text(text: &str) -> ContentBlock {
    ContentBlock::from(text)
}

fn tool_use(name: &str, id: &str) -> ContentBlock {
    ToolUseBlock::new(name, id, json!({})).into()
}

fn model(turns: Vec<Vec<ContentBlock>>) -> Arc<MockModel> {
    let model = MockModel::new();
    for turn in turns {
        model.add_turn(turn);
    }
    Arc::new(model)
}

fn counting_tool(name: &'static str) -> (FunctionTool, Arc<Mutex<Vec<Value>>>) {
    let calls = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&calls);
    let tool = FunctionTool::new(name, "A test tool", move |input, _| {
        seen.lock().unwrap().push(input);
        async move { Ok::<_, ToolError>(json!(format!("{name} ran"))) }
    });
    (tool, calls)
}

fn agent(model: &Arc<MockModel>, tools: Vec<FunctionTool>) -> Agent {
    test_agent(Arc::clone(model)).tools(tools).build().unwrap()
}

fn tool_result(message: &Message, index: usize) -> ToolResultBlock {
    match &message.content[index] {
        ContentBlock::ToolResult(result) => result.clone(),
        other => panic!("expected a tool result, got {other:?}"),
    }
}

fn content_json(result: &ToolResultBlock) -> Value {
    serde_json::to_value(&result.content).unwrap()
}

fn texts(message: &Message) -> Vec<String> {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.clone()),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn fires_hooks_during_invoke() {
    let model = model(vec![vec![text("Hello")]]);
    let mut agent = agent(&model, Vec::new());
    let recorder = HookRecorder::new();
    recorder.attach(agent.hooks());

    agent.invoke("Hi").await.unwrap();

    let events = recorder.events();
    assert_eq!(
        events[..3],
        ["initializedEvent", "beforeInvocationEvent", "messageAddedEvent"]
    );
    assert_eq!(
        events[events.len() - 3..],
        ["messageAddedEvent", "afterInvocationEvent", "agentResultEvent"]
    );
}

#[tokio::test]
async fn fires_hooks_during_stream() {
    let model = model(vec![vec![text("Hello")]]);
    let mut agent = agent(&model, Vec::new());
    let recorder = HookRecorder::new();
    recorder.attach(agent.hooks());

    {
        let mut stream = agent.stream("Hi");
        while stream.next().await.is_some() {}
    }

    assert!(recorder.events().contains(&"modelMessageEvent".to_owned()));
    assert_eq!(recorder.events().last().unwrap(), "agentResultEvent");
}

#[tokio::test]
async fn uses_messages_mutated_in_place_by_a_hook() {
    let model = model(vec![vec![text("ok")]]);
    let mut agent = agent(&model, Vec::new());
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
        event.messages_mut()[0].content = vec![TextBlock::new("rewritten").into()];
    });

    agent.invoke("original").await.unwrap();

    assert_eq!(texts(&agent.messages()[0]), ["rewritten"]);
}

#[tokio::test]
async fn uses_messages_replaced_by_a_hook() {
    let model = model(vec![vec![text("ok")]]);
    let mut agent = agent(&model, Vec::new());
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
        event.set_messages(vec![
            Message::new(Role::User, vec![text("one")]),
            Message::new(Role::User, vec![text("two")]),
        ]);
    });

    agent.invoke("original").await.unwrap();

    assert_eq!(texts(&agent.messages()[1]), ["two"]);
    assert_eq!(model.requests()[0].messages.len(), 2);
}

#[tokio::test]
async fn uses_an_empty_replacement_from_a_hook() {
    let model = model(vec![vec![text("ok")]]);
    let mut agent = test_agent(Arc::clone(&model))
        .messages(vec![Message::new(Role::User, vec![text("earlier")])])
        .build()
        .unwrap();
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_messages(Vec::new()));

    agent.invoke("dropped").await.unwrap();

    assert_eq!(model.requests()[0].messages.len(), 1);
    assert_eq!(texts(&model.requests()[0].messages[0]), ["earlier"]);
}

#[tokio::test]
async fn allows_adding_hooks_after_agent_creation_via_add_hook() {
    let model = model(vec![vec![text("one")], vec![text("two")]]);
    let mut agent = agent(&model, Vec::new());
    agent.invoke("first").await.unwrap();
    let count = Arc::new(Mutex::new(0));
    let seen = Arc::clone(&count);
    agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| *seen.lock().unwrap() += 1);

    agent.invoke("second").await.unwrap();

    assert_eq!(*count.lock().unwrap(), 1);
}

#[tokio::test]
async fn fires_after_tool_call_event_with_error_when_tool_fails() {
    let model = model(vec![vec![tool_use("bad", "t-1")], vec![text("done")]]);
    let bad = FunctionTool::new("bad", "Fails", |_, _| async move {
        Err::<Value, _>(ToolError::msg("Tool failed"))
    });
    let mut agent = test_agent(Arc::clone(&model)).tools([bad]).build().unwrap();
    let errors = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&errors);
    agent.add_hook(move |event: &mut AfterToolCallEvent| {
        seen.lock().unwrap().push(event.error().map(ToString::to_string));
    });

    agent.invoke("Go").await.unwrap();

    assert_eq!(*errors.lock().unwrap(), [Some("Tool failed".to_owned())]);
}

#[tokio::test]
async fn fires_for_initial_user_input() {
    let model = model(vec![vec![text("Hello")]]);
    let mut agent = agent(&model, Vec::new());
    let roles = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&roles);
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| seen.lock().unwrap().push(event.message().role));

    agent.invoke("Hi").await.unwrap();

    assert_eq!(*roles.lock().unwrap(), [Role::User, Role::Assistant]);
}

#[tokio::test]
async fn does_not_duplicate_user_messages_on_error_retry() {
    let model = MockModel::new();
    model.add_error(|| strands::error::ModelError::new("transient").into());
    model.add_turn(vec![text("ok")]);
    let model = Arc::new(model);
    let mut agent = test_agent(Arc::clone(&model))
        .retry_strategy(Vec::new())
        .build()
        .unwrap();
    agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
        if event.error().is_some() {
            event.set_retry(true);
        }
    });

    agent.invoke("Hi").await.unwrap();

    assert_eq!(agent.messages().len(), 2);
    assert_eq!(model.requests()[1].messages.len(), 1);
}

#[tokio::test]
async fn retries_model_call_on_success_when_hook_requests_it() {
    let model = model(vec![vec![text("first")], vec![text("second")]]);
    let mut agent = agent(&model, Vec::new());
    let retried = Arc::new(Mutex::new(false));
    let once = Arc::clone(&retried);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        let mut retried = once.lock().unwrap();
        if !*retried {
            *retried = true;
            event.set_retry(true);
        }
    });

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.to_string(), "second");
    assert_eq!(agent.messages().len(), 2);
}

#[tokio::test]
async fn retries_tool_call_when_hook_sets_retry() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("done")]]);
    let (echo, calls) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    let attempts = Arc::new(Mutex::new(0));
    let seen = Arc::clone(&attempts);
    agent.add_hook(move |event: &mut AfterToolCallEvent| {
        let mut attempts = seen.lock().unwrap();
        *attempts += 1;
        if *attempts == 1 {
            event.set_retry(true);
        }
    });

    agent.invoke("Go").await.unwrap();

    assert_eq!(calls.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn does_not_retry_a_tool_call_after_agent_cancellation() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("done")]]);
    let (echo, calls) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut AfterToolCallEvent| {
        event.agent().cancel_handle().cancel(CancelOptions::default());
        event.set_retry(true);
    });

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(calls.lock().unwrap().len(), 1);
    assert_eq!(result.stop_reason(), StopReason::Cancelled);
}

#[tokio::test]
async fn fires_before_tool_call_event_on_each_retry() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("done")]]);
    let (echo, _) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    let before = Arc::new(Mutex::new(0));
    let seen = Arc::clone(&before);
    agent.add_hook(move |_: &mut BeforeToolCallEvent| *seen.lock().unwrap() += 1);
    let retried = Arc::new(Mutex::new(false));
    let once = Arc::clone(&retried);
    agent.add_hook(move |event: &mut AfterToolCallEvent| {
        let mut retried = once.lock().unwrap();
        if !*retried {
            *retried = true;
            event.set_retry(true);
        }
    });

    agent.invoke("Go").await.unwrap();

    assert_eq!(*before.lock().unwrap(), 2);
}

#[tokio::test]
async fn cancels_individual_tool_call_with_default_message_when_cancel_is_true() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("done")]]);
    let (echo, calls) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel(Some(Cancel::new())));

    agent.invoke("Go").await.unwrap();

    assert!(calls.lock().unwrap().is_empty());
    let result = tool_result(&agent.messages()[2], 0);
    assert_eq!(result.status, ToolResultStatus::Error);
    assert_eq!(
        content_json(&result),
        json!([{ "text": "Tool cancelled by hook" }])
    );
}

#[tokio::test]
async fn cancels_only_specific_tools_when_before_tool_call_event_selectively_cancels() {
    let model = model(vec![
        vec![tool_use("keep", "t-1"), tool_use("drop", "t-2")],
        vec![text("done")],
    ]);
    let (keep, kept) = counting_tool("keep");
    let (drop_tool, dropped) = counting_tool("drop");
    let mut agent = agent(&model, vec![keep, drop_tool]);
    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name == "drop" {
            event.set_cancel(Some("not allowed".into()));
        }
    });

    agent.invoke("Go").await.unwrap();

    assert_eq!(kept.lock().unwrap().len(), 1);
    assert!(dropped.lock().unwrap().is_empty());
    assert_eq!(
        tool_result(&agent.messages()[2], 0).status,
        ToolResultStatus::Success
    );
    assert_eq!(
        content_json(&tool_result(&agent.messages()[2], 1)),
        json!([{ "text": "not allowed" }])
    );
}

#[tokio::test]
async fn cancels_all_tools_in_a_batch_via_before_tools_event_with_correct_tool_use_ids() {
    let model = model(vec![
        vec![tool_use("a", "t-1"), tool_use("b", "t-2")],
        vec![text("done")],
    ]);
    let (a, a_calls) = counting_tool("a");
    let (b, b_calls) = counting_tool("b");
    let mut agent = agent(&model, vec![a, b]);
    agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(Some("batch denied".into())));

    agent.invoke("Go").await.unwrap();

    assert!(a_calls.lock().unwrap().is_empty());
    assert!(b_calls.lock().unwrap().is_empty());
    let message = &agent.messages()[2];
    assert_eq!(tool_result(message, 0).tool_use_id, "t-1");
    assert_eq!(tool_result(message, 1).tool_use_id, "t-2");
    assert_eq!(
        content_json(&tool_result(message, 1)),
        json!([{ "text": "batch denied" }])
    );
}

#[tokio::test]
async fn allows_hooks_to_replace_result_on_after_tool_call_event() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("done")]]);
    let (echo, _) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut AfterToolCallEvent| {
        let id = event.tool_use().tool_use_id.clone();
        event.set_result(ToolResultBlock::new(
            id,
            ToolResultStatus::Success,
            vec!["replaced".into()],
        ));
    });

    agent.invoke("Go").await.unwrap();

    assert_eq!(
        content_json(&tool_result(&agent.messages()[2], 0)),
        json!([{ "text": "replaced" }])
    );
}

#[tokio::test]
async fn halts_the_loop_with_custom_assistant_message_when_end_turn_is_a_string() {
    let model = model(vec![vec![tool_use("echo", "t-1")]]);
    let (echo, _) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut AfterToolsEvent<'_>| {
        event.set_end_turn(Some(EndTurn::Message("Stopping here".into())));
    });

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(texts(result.last_message()), ["Stopping here"]);
}

#[tokio::test]
async fn treats_empty_string_end_turn_as_falsy_does_not_halt() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("continued")]]);
    let (echo, _) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut AfterToolsEvent<'_>| {
        event.set_end_turn(Some(EndTurn::Message(String::new())));
    });

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(texts(result.last_message()), ["continued"]);
}

#[tokio::test]
async fn treats_empty_array_end_turn_as_falsy_does_not_halt() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("continued")]]);
    let (echo, _) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(Some(EndTurn::Content(Vec::new()))));

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(texts(result.last_message()), ["continued"]);
}

#[tokio::test]
async fn halts_with_content_blocks_when_end_turn_is_a_content_block_array() {
    let model = model(vec![vec![tool_use("echo", "t-1")]]);
    let (echo, _) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut AfterToolsEvent<'_>| {
        event.set_end_turn(Some(EndTurn::Content(vec![text("a"), text("b")])));
    });

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(texts(result.last_message()), ["a", "b"]);
}

#[tokio::test]
async fn halts_the_loop_with_concurrent_tool_execution() {
    let model = model(vec![vec![tool_use("a", "t-1"), tool_use("b", "t-2")]]);
    let (a, _) = counting_tool("a");
    let (b, _) = counting_tool("b");
    let mut agent = test_agent(Arc::clone(&model))
        .tools([a, b])
        .tool_executor(ToolExecutorStrategy::Concurrent)
        .build()
        .unwrap();
    agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(Some(EndTurn::DefaultMessage)));

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(model.requests().len(), 1);
}

#[tokio::test]
async fn halts_even_when_set_on_a_cancelled_tools_after_tools_event() {
    let model = model(vec![vec![tool_use("echo", "t-1")]]);
    let (echo, _) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(Some(Cancel::new())));
    agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(Some(EndTurn::DefaultMessage)));

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(
        texts(result.last_message()),
        ["Turn ended early by hook after tool execution"]
    );
}

#[tokio::test]
async fn cancels_invocation_with_custom_message_when_cancel_is_a_string() {
    let model = model(vec![vec![text("unused")]]);
    let mut agent = agent(&model, Vec::new());
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_cancel(Some("not today".into())));

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(texts(result.last_message()), ["not today"]);
    assert!(model.requests().is_empty());
}

#[tokio::test]
async fn does_not_append_user_message_when_invocation_is_cancelled() {
    let model = model(vec![vec![text("unused")]]);
    let mut agent = agent(&model, Vec::new());
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_cancel(Some(Cancel::new())));

    agent.invoke("Hi").await.unwrap();

    assert_eq!(agent.messages().len(), 1);
    assert_eq!(texts(&agent.messages()[0]), ["invocation denied by hook"]);
}

#[tokio::test]
async fn emits_after_invocation_event_when_invocation_is_cancelled() {
    let model = model(vec![vec![text("unused")]]);
    let mut agent = agent(&model, Vec::new());
    let recorder = HookRecorder::new();
    recorder.attach(agent.hooks());
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_cancel(Some(Cancel::new())));

    agent.invoke("Hi").await.unwrap();

    assert_eq!(recorder.events().last().unwrap(), "afterInvocationEvent");
}

#[tokio::test]
async fn cancels_model_call_with_default_message_when_cancel_is_true() {
    let model = model(vec![vec![text("unused")]]);
    let mut agent = agent(&model, Vec::new());
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.set_cancel(Some(Cancel::new())));

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(texts(result.last_message()), ["model call denied by hook"]);
    assert!(model.requests().is_empty());
}

#[tokio::test]
async fn does_not_emit_model_message_event_when_model_call_is_cancelled() {
    let model = model(vec![vec![text("unused")]]);
    let mut agent = agent(&model, Vec::new());
    let fired = Arc::new(Mutex::new(false));
    let seen = Arc::clone(&fired);
    agent.add_hook(move |_: &mut ModelMessageEvent<'_>| *seen.lock().unwrap() = true);
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.set_cancel(Some(Cancel::new())));

    agent.invoke("Hi").await.unwrap();

    assert!(!*fired.lock().unwrap());
}

#[tokio::test]
async fn invokes_the_replacement_tool_instead_of_the_registry_tool() {
    let model = model(vec![vec![tool_use("original", "t-1")], vec![text("done")]]);
    let (original, original_calls) = counting_tool("original");
    let (replacement, replacement_calls) = counting_tool("replacement");
    let replacement: Arc<dyn Tool> = Arc::new(replacement);
    let mut agent = agent(&model, vec![original]);
    agent.add_hook(move |event: &mut BeforeToolCallEvent| {
        event.set_selected_tool(Some(Arc::clone(&replacement)));
    });

    agent.invoke("Go").await.unwrap();

    assert!(original_calls.lock().unwrap().is_empty());
    assert_eq!(replacement_calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn passes_mutated_input_to_the_tool() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("done")]]);
    let (echo, calls) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut BeforeToolCallEvent| event.tool_use_mut().input = json!({ "patched": true }));

    agent.invoke("Go").await.unwrap();

    assert_eq!(*calls.lock().unwrap(), [json!({ "patched": true })]);
}

#[tokio::test]
async fn re_resolves_the_tool_when_hook_renames_tool_use_name() {
    let model = model(vec![vec![tool_use("first", "t-1")], vec![text("done")]]);
    let (first, first_calls) = counting_tool("first");
    let (second, second_calls) = counting_tool("second");
    let mut agent = agent(&model, vec![first, second]);
    agent.add_hook(|event: &mut BeforeToolCallEvent| "second".clone_into(&mut event.tool_use_mut().name));

    agent.invoke("Go").await.unwrap();

    assert!(first_calls.lock().unwrap().is_empty());
    assert_eq!(second_calls.lock().unwrap().len(), 1);
    assert_eq!(tool_result(&agent.messages()[2], 0).tool_use_id, "t-1");
}

#[tokio::test]
async fn propagates_mutated_result_into_after_tools_event() {
    let model = model(vec![vec![tool_use("echo", "t-1")], vec![text("done")]]);
    let (echo, _) = counting_tool("echo");
    let mut agent = agent(&model, vec![echo]);
    agent.add_hook(|event: &mut AfterToolCallEvent| {
        let id = event.tool_use().tool_use_id.clone();
        event.set_result(ToolResultBlock::new(
            id,
            ToolResultStatus::Success,
            vec!["mutated".into()],
        ));
    });
    let seen = Arc::new(Mutex::new(None));
    let record = Arc::clone(&seen);
    agent.add_hook(move |event: &mut AfterToolsEvent<'_>| {
        *record.lock().unwrap() = Some(content_json(&tool_result(event.message(), 0)));
    });

    agent.invoke("Go").await.unwrap();

    assert_eq!(*seen.lock().unwrap(), Some(json!([{ "text": "mutated" }])));
}

#[tokio::test]
async fn chains_multiple_resumes() {
    let model = model(vec![vec![text("one")], vec![text("two")], vec![text("three")]]);
    let mut agent = agent(&model, Vec::new());
    let remaining = Arc::new(Mutex::new(2));
    let left = Arc::clone(&remaining);
    agent.add_hook(move |event: &mut strands::hooks::AfterInvocationEvent<'_>| {
        let mut left = left.lock().unwrap();
        if *left > 0 {
            *left -= 1;
            event.set_resume(Some("again".into()));
        }
    });

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.to_string(), "three");
    assert_eq!(agent.messages().len(), 6);
}
