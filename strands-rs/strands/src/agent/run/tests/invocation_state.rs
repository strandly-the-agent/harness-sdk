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
async fn returns_an_empty_object_on_agent_result_when_no_invocation_state_is_passed() {
    let result = test_agent(one_turn()).invoke("Hi").await.unwrap();
    assert!(result.invocation_state().is_empty());
}

#[tokio::test]
async fn returns_the_passed_invocation_state_on_agent_result() {
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
async fn propagates_mutations_from_before_model_call_event_to_after_model_call_event_and_agent_result() {
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
async fn shares_the_same_invocation_state_object_across_all_lifecycle_events_in_one_invocation() {
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
async fn persists_mutations_across_recursive_agent_loop_cycles_tool_use_scenario() {
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
async fn passes_invocation_state_to_tools_via_tool_context_and_surfaces_mutations_on_the_result() {
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
async fn does_not_touch_agent_app_state_when_invocation_state_is_mutated() {
    let mut agent = test_agent(one_turn());
    agent.app_state().set("persistent", "yes").unwrap();
    agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
        event.invocation_state().insert(Marker("only-this-run"));
    });
    agent.invoke("Hi").await.unwrap();
    assert_eq!(agent.app_state().keys(), vec!["persistent".to_owned()]);
}

#[tokio::test]
async fn does_not_leak_state_between_invocations_on_the_same_agent_default_bag() {
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
async fn preserves_same_invocation_state_reference_across_after_model_call_event_retry() {
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
async fn preserves_same_invocation_state_reference_across_after_tool_call_event_retry() {
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
