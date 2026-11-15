//! Ported from strands-ts/src/agent/__tests__/agent.invocation-state.test.ts. `InvocationState`
//! is a shared handle, so "the same object" means a write through one handle is visible
//! through every other.

use super::*;
use crate::hooks::{AfterToolCallEvent, BeforeModelCallEvent};
use crate::tools::ToolContext;

#[derive(Clone, Debug, PartialEq)]
struct Counter(u32);

#[derive(Clone, Debug, PartialEq)]
struct Marker(&'static str);

fn one_turn() -> Arc<MockMessageModel> {
    mock(vec![vec![text("Hello")]])
}

#[tokio::test]
async fn the_result_has_an_empty_state_when_none_is_passed() {
    let result = test_agent(one_turn()).invoke("Hi").await.unwrap();
    assert!(result.invocation_state().is_empty());
}

#[tokio::test]
async fn the_result_carries_the_passed_state_and_the_caller_keeps_the_same_handle() {
    let state = InvocationState::new();
    state.insert(Marker("caller"));
    let options = InvokeOptions {
        invocation_state: state.clone(),
        ..InvokeOptions::default()
    };
    let result = test_agent(one_turn()).invoke_with("Hi", options).await.unwrap();
    assert_eq!(result.invocation_state().get::<Marker>(), Some(Marker("caller")));
    result.invocation_state().insert(Counter(7));
    assert_eq!(state.get::<Counter>(), Some(Counter(7)));
}

#[tokio::test]
async fn mutations_from_before_model_call_reach_after_model_call_and_the_result() {
    let mut agent = test_agent(one_turn());
    agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
        let next = event
            .invocation_state()
            .get::<Counter>()
            .map_or(1, |Counter(n)| n + 1);
        event.invocation_state().insert(Counter(next));
    });
    let seen = Arc::new(Mutex::new(None));
    let sink = Arc::clone(&seen);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        *sink.lock().unwrap() = event.invocation_state().get::<Counter>();
    });
    let result = agent.invoke("Hi").await.unwrap();
    assert_eq!(*seen.lock().unwrap(), Some(Counter(1)));
    assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(1)));
}

#[tokio::test]
async fn every_lifecycle_event_shares_one_state() {
    let mut agent = test_agent(one_turn());
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
        event.invocation_state().insert(Marker("shared"));
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        sink.lock()
            .unwrap()
            .push(event.invocation_state().get::<Marker>());
    });
    let sink = Arc::clone(&seen);
    agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
        sink.lock()
            .unwrap()
            .push(event.invocation_state().get::<Marker>());
    });
    let sink = Arc::clone(&seen);
    agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
        sink.lock()
            .unwrap()
            .push(event.invocation_state().get::<Marker>());
    });
    let sink = Arc::clone(&seen);
    agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
        sink.lock()
            .unwrap()
            .push(event.invocation_state().get::<Marker>());
    });
    agent.invoke("Hi").await.unwrap();
    let seen = seen.lock().unwrap();
    assert!(seen.len() >= 4);
    assert!(seen.iter().all(|marker| *marker == Some(Marker("shared"))));
}

#[tokio::test]
async fn mutations_persist_across_cycles() {
    let model = mock(vec![
        vec![tool_use("add", "tool-1", json!({}))],
        vec![text("Done")],
    ]);
    let mut agent = tool_agent(&model);
    agent.add_hook(|event: &mut AfterToolCallEvent| {
        event.invocation_state().insert(Marker("tool completed"));
    });
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        sink.lock()
            .unwrap()
            .push(event.invocation_state().get::<Marker>());
    });
    let result = agent.invoke("Run").await.unwrap();
    assert_eq!(*seen.lock().unwrap(), vec![None, Some(Marker("tool completed"))]);
    assert_eq!(
        result.invocation_state().get::<Marker>(),
        Some(Marker("tool completed"))
    );
}

#[tokio::test]
async fn tools_get_the_state_and_their_writes_reach_the_result() {
    let tool = FunctionTool::new(
        "lookup",
        "reads the user",
        |_input: JsonValue, ctx: ToolContext| async move {
            let user = ctx.invocation_state().get::<Marker>().unwrap();
            ctx.invocation_state().insert(Counter(1));
            Ok::<_, ToolError>(user.0)
        },
    );
    let model = mock(vec![vec![tool_use("lookup", "t1", json!({}))], vec![text("ok")]]);
    let mut agent = Agent::builder().model(model).tools([tool]).build().unwrap();
    let state = InvocationState::new();
    state.insert(Marker("u-1"));
    let options = InvokeOptions {
        invocation_state: state,
        ..InvokeOptions::default()
    };
    let result = agent.invoke_with("go", options).await.unwrap();
    assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(1)));
    let tool_result = agent.messages()[2].content()[0].clone();
    let ContentBlock::ToolResult(tool_result) = tool_result else {
        panic!("expected a tool result");
    };
    assert_eq!(tool_result.status(), &ToolResultStatus::Success);
}

#[tokio::test]
async fn state_writes_do_not_touch_app_state() {
    let mut agent = test_agent(one_turn());
    agent.app_state().set("persistent", "yes").unwrap();
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
        event.invocation_state().insert(Marker("only-this-run"));
    });
    agent.invoke("Hi").await.unwrap();
    assert_eq!(agent.app_state().keys(), vec!["persistent".to_owned()]);
}

#[tokio::test]
async fn the_default_state_does_not_leak_between_invocations() {
    let mut agent = test_agent(one_turn());
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
        assert!(event.invocation_state().get::<Marker>().is_none());
        event.invocation_state().insert(Marker("seen"));
    });
    let first = agent.invoke("one").await.unwrap();
    let second = agent.invoke("two").await.unwrap();
    first.invocation_state().insert(Counter(1));
    assert_eq!(second.invocation_state().get::<Counter>(), None);
}

#[tokio::test]
async fn an_after_model_call_retry_keeps_the_same_state() {
    let model = Arc::new(MockMessageModel::new());
    model.add_error(|| ModelError::new("transient").into());
    model.add_turn(vec![text("ok")]);
    let mut agent = test_agent(model);
    agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
        let next = event
            .invocation_state()
            .get::<Counter>()
            .map_or(1, |Counter(n)| n + 1);
        event.invocation_state().insert(Counter(next));
        if event.error().is_some() {
            event.set_retry(true);
        }
    });
    let state = InvocationState::new();
    let options = InvokeOptions {
        invocation_state: state.clone(),
        ..InvokeOptions::default()
    };
    agent.invoke_with("Hi", options).await.unwrap();
    assert_eq!(state.get::<Counter>(), Some(Counter(2)));
}

#[tokio::test]
async fn an_after_tool_call_retry_keeps_the_same_state() {
    let model = mock(vec![
        vec![tool_use("add", "tool-1", json!({}))],
        vec![text("Done")],
    ]);
    let mut agent = tool_agent(&model);
    agent.add_hook(|event: &mut AfterToolCallEvent| {
        let next = event
            .invocation_state()
            .get::<Counter>()
            .map_or(1, |Counter(n)| n + 1);
        event.invocation_state().insert(Counter(next));
        if next == 1 {
            event.set_retry(true);
        }
    });
    let state = InvocationState::new();
    let options = InvokeOptions {
        invocation_state: state.clone(),
        ..InvokeOptions::default()
    };
    agent.invoke_with("Run", options).await.unwrap();
    assert_eq!(state.get::<Counter>(), Some(Counter(2)));
}
