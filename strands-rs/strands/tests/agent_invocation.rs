//! Ported from strands-ts/src/agent/__tests__/agent.invocation-state.test.ts and
//! agent.invocation-context.test.ts

use std::num::NonZeroU64;
use std::sync::{Arc, Mutex};

use serde_json::json;
use strands::agent::InvokeLimits;
use strands::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, BeforeInvocationEvent,
    BeforeModelCallEvent,
};
use strands::models::streaming::Usage;
use strands::testing::{MockModel, MockTurnOptions, test_agent};
use strands::tools::FunctionTool;
use strands::types::messages::ToolUseBlock;
use strands::{ContentBlock, InvocationState, InvokeOptions, StopReason, ToolError};

#[derive(Clone, Debug, PartialEq)]
struct Counter(u32);

fn replying(text: &str) -> MockModel {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from(text)]);
    model
}

fn with_usage(model: &MockModel, content: Vec<ContentBlock>, usage: Usage) {
    model.add_turn_with(
        content,
        MockTurnOptions {
            usage: Some(usage),
            ..MockTurnOptions::default()
        },
    );
}

fn state_options(state: &InvocationState) -> InvokeOptions {
    InvokeOptions {
        invocation_state: Some(state.clone()),
        ..InvokeOptions::default()
    }
}

fn limits(turns: Option<u64>, total_tokens: Option<u64>) -> Option<InvokeLimits> {
    Some(InvokeLimits {
        turns: turns.and_then(NonZeroU64::new),
        total_tokens: total_tokens.and_then(NonZeroU64::new),
        ..InvokeLimits::default()
    })
}

#[tokio::test]
async fn returns_an_empty_object_on_agent_result_when_no_invocation_state_is_passed() {
    let mut agent = test_agent(replying("hi")).build().unwrap();

    let result = agent.invoke("Hello").await.unwrap();

    assert!(!result.invocation_state().contains::<Counter>());
}

#[tokio::test]
async fn returns_the_passed_invocation_state_on_agent_result() {
    let mut agent = test_agent(replying("hi")).build().unwrap();
    let state = InvocationState::new();
    state.insert(Counter(1));

    let result = agent.invoke_with("Hello", state_options(&state)).await.unwrap();

    assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(1)));
}

#[tokio::test]
async fn propagates_mutations_from_before_model_call_event_to_after_model_call_event_and_agent_result() {
    let mut agent = test_agent(replying("hi")).build().unwrap();
    let seen = Arc::new(Mutex::new(None));
    let record = Arc::clone(&seen);
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
        event.invocation_state().insert(Counter(7));
    });
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        *record.lock().unwrap() = event.invocation_state().get::<Counter>();
    });
    let state = InvocationState::new();

    let result = agent.invoke_with("Hello", state_options(&state)).await.unwrap();

    assert_eq!(*seen.lock().unwrap(), Some(Counter(7)));
    assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(7)));
    assert_eq!(state.get::<Counter>(), Some(Counter(7)));
}

#[tokio::test]
async fn persists_mutations_across_recursive_agent_loop_cycles_tool_use_scenario() {
    let model = MockModel::new();
    model.add_turn(vec![ToolUseBlock::new("bump", "t-1", json!({})).into()]);
    model.add_turn(vec![ContentBlock::from("done")]);
    let bump = FunctionTool::new("bump", "Increments the counter", |_, context| {
        let state = context.invocation_state().clone();
        async move {
            let next = state.get::<Counter>().map_or(1, |counter| counter.0 + 1);
            state.insert(Counter(next));
            Ok::<_, ToolError>(json!(next))
        }
    });
    let mut agent = test_agent(model).tools([bump]).build().unwrap();
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
        let next = event
            .invocation_state()
            .get::<Counter>()
            .map_or(10, |counter| counter.0 + 10);
        event.invocation_state().insert(Counter(next));
    });

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(21)));
}

#[tokio::test]
async fn does_not_touch_agent_app_state_when_invocation_state_is_mutated() {
    let mut agent = test_agent(replying("hi")).build().unwrap();
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
        event.invocation_state().insert(Counter(1));
    });

    agent.invoke("Hello").await.unwrap();

    assert!(agent.app_state().keys().is_empty());
}

#[tokio::test]
async fn does_not_leak_state_between_invocations_on_the_same_agent_default_bag() {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from("one")]);
    model.add_turn(vec![ContentBlock::from("two")]);
    let mut agent = test_agent(model).build().unwrap();
    let seen = Arc::new(Mutex::new(Vec::new()));
    let record = Arc::clone(&seen);
    agent.add_hook(move |event: &mut BeforeInvocationEvent<'_>| {
        record
            .lock()
            .unwrap()
            .push(event.invocation_state().get::<Counter>());
        event.invocation_state().insert(Counter(1));
    });

    agent.invoke("first").await.unwrap();
    agent.invoke("second").await.unwrap();

    assert_eq!(*seen.lock().unwrap(), [None, None]);
}

#[tokio::test]
async fn preserves_same_invocation_state_reference_across_after_tool_call_event_retry() {
    let model = MockModel::new();
    model.add_turn(vec![ToolUseBlock::new("echo", "t-1", json!({})).into()]);
    model.add_turn(vec![ContentBlock::from("done")]);
    let echo = FunctionTool::new("echo", "Echoes", |_, context| {
        let state = context.invocation_state().clone();
        async move {
            let calls = state.get::<Counter>().map_or(1, |counter| counter.0 + 1);
            state.insert(Counter(calls));
            Ok::<_, ToolError>(json!(calls))
        }
    });
    let mut agent = test_agent(model).tools([echo]).build().unwrap();
    agent.add_hook(|event: &mut AfterToolCallEvent| {
        if event.invocation_state().get::<Counter>() == Some(Counter(1)) {
            event.set_retry(true);
        }
    });

    let result = agent.invoke("Go").await.unwrap();

    assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(2)));
}

#[tokio::test]
async fn produces_a_well_formed_result_when_an_inherited_limit_is_already_exhausted_on_entry() {
    let first_model = MockModel::new();
    first_model.add_turn(vec![ContentBlock::from("one")]);
    let mut first = test_agent(first_model).build().unwrap();
    let request = first
        .invoke_with(
            "Hi",
            InvokeOptions {
                limits: limits(Some(1), None),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap()
        .invocation_context()
        .cloned()
        .unwrap();
    let second_model = Arc::new(replying("unreachable"));
    let mut second = test_agent(Arc::clone(&second_model)).build().unwrap();

    let result = second
        .invoke_with(
            "Again",
            InvokeOptions {
                invocation_context: Some(request),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason(), StopReason::LimitTurns);
    assert_eq!(
        serde_json::to_value(result.last_message()).unwrap()["role"],
        "assistant"
    );
    assert!(result.last_message().content.is_empty());
    assert_eq!(result.to_string(), "");
    assert!(second_model.requests().is_empty());
}

#[tokio::test]
async fn continues_a_previous_calls_limits_and_usage_when_passed_its_invocation_context() {
    let model = MockModel::new();
    with_usage(&model, vec![ContentBlock::from("one")], Usage::new(10, 5, 15));
    with_usage(&model, vec![ContentBlock::from("two")], Usage::new(10, 5, 15));
    let mut agent = test_agent(model).build().unwrap();
    let first = agent
        .invoke_with(
            "Hi",
            InvokeOptions {
                limits: limits(Some(5), None),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();
    let context = first.invocation_context().cloned().unwrap();

    let second = agent
        .invoke_with(
            "Again",
            InvokeOptions {
                invocation_context: Some(context.clone()),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(
        second.invocation_context().unwrap().usage().usage,
        Usage::new(20, 10, 30)
    );
    assert_eq!(context.usage().turns_taken, 2);
}

#[tokio::test]
async fn stops_a_nested_invoke_on_the_requests_limit_even_when_its_own_limit_is_looser() {
    let mut outer = test_agent(replying("outer")).build().unwrap();
    let outer_result = outer
        .invoke_with(
            "Hi",
            InvokeOptions {
                limits: limits(Some(1), None),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();
    let shared = outer_result.invocation_context().cloned().unwrap();
    let nested_model = Arc::new(replying("nested"));
    let mut nested = test_agent(Arc::clone(&nested_model)).build().unwrap();

    let result = nested
        .invoke_with(
            "Nested",
            InvokeOptions {
                invocation_context: Some(shared),
                limits: limits(Some(10), None),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(result.stop_reason(), StopReason::LimitTurns);
    assert!(nested_model.requests().is_empty());
}

#[tokio::test]
async fn starts_fresh_request_state_on_each_root_invoke_of_a_reused_agent() {
    let model = MockModel::new();
    with_usage(&model, vec![ContentBlock::from("one")], Usage::new(1, 1, 2));
    with_usage(&model, vec![ContentBlock::from("two")], Usage::new(1, 1, 2));
    let mut agent = test_agent(model).build().unwrap();

    agent.invoke("first").await.unwrap();
    let second = agent.invoke("second").await.unwrap();

    assert_eq!(
        second.invocation_context().unwrap().usage().usage,
        Usage::new(1, 1, 2)
    );
    assert_eq!(second.invocation_context().unwrap().usage().turns_taken, 1);
}

#[tokio::test]
async fn ends_on_a_limit_stop_even_when_a_hook_asks_to_resume() {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from("one")]);
    model.add_turn(vec![ContentBlock::from("two")]);
    let model = Arc::new(model);
    let mut agent = test_agent(Arc::clone(&model)).build().unwrap();
    agent.add_hook(|event: &mut AfterInvocationEvent<'_>| event.set_resume(Some("again".into())));

    let result = agent
        .invoke_with(
            "Hi",
            InvokeOptions {
                limits: limits(Some(1), None),
                ..InvokeOptions::default()
            },
        )
        .await
        .unwrap();

    assert_eq!(model.requests().len(), 1);
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
}

#[tokio::test]
async fn resume_reenters_the_loop_with_the_new_input() {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::from("one")]);
    model.add_turn(vec![ContentBlock::from("two")]);
    let model = Arc::new(model);
    let mut agent = test_agent(Arc::clone(&model)).build().unwrap();
    let resumed = Arc::new(Mutex::new(false));
    let once = Arc::clone(&resumed);
    agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
        let mut resumed = once.lock().unwrap();
        if !*resumed {
            *resumed = true;
            event.set_resume(Some("follow-up prompt".into()));
        }
    });

    let result = agent.invoke("Hi").await.unwrap();

    assert_eq!(result.to_string(), "two");
    assert_eq!(agent.messages().len(), 4);
    assert_eq!(model.requests()[1].messages.len(), 3);
}
