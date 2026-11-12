//! Ported from strands-ts/src/agent/__tests__/invocation-context.test.ts,
//! strands-ts/src/agent/__tests__/agent.invocation-context.test.ts and
//! strands-ts/src/agent/__tests__/agent.invocation-state.test.ts
//!
//! `InvocationState` is a typed map here, so the TS frozen-object and plain-object identity
//! cases become `ptr_eq` checks; a context the SDK did not create can't be built.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Agent, AgentConfig, InvocationContext, InvocationState, InvokeLimits, InvokeOptions};
use crate::hooks::{AfterInvocationEvent, AfterModelCallEvent, BeforeInvocationEvent, BeforeModelCallEvent};
use crate::models::{Model, Usage};
use crate::testing::{MockModel, mock_tool, text_result};
use crate::tools::ToolSource;
use crate::types::{ContentBlock, StopReason, ToolUseBlock};

fn agent(model: &Arc<MockModel>, tools: Vec<ToolSource>) -> Agent {
    Agent::new(AgentConfig {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        tools,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn limits(turns: Option<u64>, output_tokens: Option<u64>) -> Option<InvokeLimits> {
    Some(InvokeLimits {
        turns,
        output_tokens,
        ..Default::default()
    })
}

mod context {
    use super::*;

    #[test]
    fn counts_toward_limiting_parents_but_not_through_an_auxiliary_call() {
        let root = InvocationContext::resolve(None, limits(Some(5), None)).unwrap();
        let child = InvocationContext::resolve(Some(&root), limits(Some(9), None)).unwrap();
        child.record_turn();
        child.record_usage(&Usage::new(1, 2, 3));
        assert_eq!(root.usage().turns_taken, 1);
        assert_eq!(root.usage().usage, Usage::new(1, 2, 3));
        let aux = InvocationContext::auxiliary(Some(&root)).unwrap();
        aux.record_turn();
        aux.record_usage(&Usage::new(1, 1, 2));
        assert_eq!(root.usage().turns_taken, 1);
        assert_eq!(root.usage().usage, Usage::new(2, 3, 5));
    }

    #[test]
    fn sees_the_limits_of_limiting_parents_only() {
        let root = InvocationContext::resolve(None, limits(Some(1), None)).unwrap();
        let child = InvocationContext::resolve(Some(&root), limits(None, Some(100))).unwrap();
        root.record_turn();
        assert_eq!(child.reached_limit(), Some(StopReason::LimitTurns));
        let aux = InvocationContext::auxiliary(Some(&root)).unwrap();
        assert_eq!(aux.reached_limit(), None);
    }
}

mod agent_context {
    use super::*;

    fn looping(model: &Arc<MockModel>) -> Agent {
        model.add_turn_with(
            vec![ContentBlock::ToolUse(ToolUseBlock::new("calc", "t1", json!({})))],
            None,
            Some(Usage::new(10, 10, 20)),
        );
        agent(
            model,
            crate::tool_list![mock_tool("calc", |ctx| text_result(ctx, "4"))],
        )
    }

    #[tokio::test]
    async fn produces_a_well_formed_result_when_an_inherited_limit_is_already_exhausted_on_entry() {
        let root = InvocationContext::resolve(None, limits(Some(1), None)).unwrap();
        root.record_turn();
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["unused".into()]);
        let mut agent = agent(&model, Vec::new());
        let options = InvokeOptions {
            invocation_context: Some(root),
            ..Default::default()
        };
        let result = agent.invoke_with("Hi", options).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert!(model.requests().is_empty());
        assert!(result.last_message().content.is_empty());
    }

    #[tokio::test]
    async fn rejects_invalid_limits_before_any_invocation_hook_runs() {
        let model = Arc::new(MockModel::new());
        let mut agent = agent(&model, Vec::new());
        let fired = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&fired);
        agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| *flag.lock().unwrap() = true);
        let options = InvokeOptions {
            limits: limits(None, Some(0)),
            ..Default::default()
        };
        let error = agent.invoke_with("Hi", options).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "limits.outputTokens must be a positive finite number, got 0"
        );
        assert!(!*fired.lock().unwrap());
    }

    #[tokio::test]
    async fn reports_the_whole_request_s_usage_on_the_result_while_metrics_cover_this_agent_only() {
        let root = InvocationContext::resolve(None, None).unwrap();
        root.record_usage(&Usage::new(100, 100, 200));
        let model = Arc::new(MockModel::new());
        model.add_turn_with(vec!["ok".into()], None, Some(Usage::new(1, 1, 2)));
        let mut agent = agent(&model, Vec::new());
        let options = InvokeOptions {
            invocation_context: Some(root),
            ..Default::default()
        };
        let result = agent.invoke_with("Hi", options).await.unwrap();
        assert_eq!(
            result.invocation_context().unwrap().usage().usage,
            Usage::new(101, 101, 202)
        );
        assert_eq!(result.metrics().accumulated_usage, Usage::new(1, 1, 2));
    }

    #[tokio::test]
    async fn continues_a_previous_call_s_limits_and_usage_when_passed_its_invocation_context() {
        let model = Arc::new(MockModel::new());
        let mut agent = looping(&model);
        let options = InvokeOptions {
            limits: limits(Some(2), None),
            ..Default::default()
        };
        let first = agent.invoke_with("go", options).await.unwrap();
        assert_eq!(first.stop_reason(), StopReason::LimitTurns);
        let context = first.invocation_context().cloned();
        let options = InvokeOptions {
            invocation_context: context,
            ..Default::default()
        };
        let second = agent.invoke_with("again", options).await.unwrap();
        assert_eq!(second.stop_reason(), StopReason::LimitTurns);
        assert_eq!(model.requests().len(), 2);
    }

    #[tokio::test]
    async fn stops_a_nested_invoke_on_the_request_s_limit_even_when_its_own_limit_is_looser() {
        let root = InvocationContext::resolve(None, limits(Some(1), None)).unwrap();
        let model = Arc::new(MockModel::new());
        let mut agent = looping(&model);
        let options = InvokeOptions {
            invocation_context: Some(root.clone()),
            limits: limits(Some(10), None),
            ..Default::default()
        };
        let result = agent.invoke_with("go", options).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(root.usage().turns_taken, 1);
    }

    #[tokio::test]
    async fn stops_a_nested_invoke_on_its_own_tighter_limit_and_counts_its_turns_toward_the_request() {
        let root = InvocationContext::resolve(None, limits(Some(10), None)).unwrap();
        let model = Arc::new(MockModel::new());
        let mut agent = looping(&model);
        let options = InvokeOptions {
            invocation_context: Some(root.clone()),
            limits: limits(Some(2), None),
            ..Default::default()
        };
        let result = agent.invoke_with("go", options).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(root.usage().turns_taken, 2);
    }

    #[tokio::test]
    async fn counts_only_a_nested_invoke_s_own_tokens_against_its_own_token_limits() {
        let root = InvocationContext::resolve(None, None).unwrap();
        root.record_usage(&Usage::new(0, 1000, 1000));
        let model = Arc::new(MockModel::new());
        model.add_turn_with(vec!["ok".into()], None, Some(Usage::new(1, 1, 2)));
        let mut agent = agent(&model, Vec::new());
        let options = InvokeOptions {
            invocation_context: Some(root),
            limits: limits(None, Some(500)),
            ..Default::default()
        };
        assert_eq!(
            agent.invoke_with("Hi", options).await.unwrap().stop_reason(),
            StopReason::EndTurn
        );
    }

    #[tokio::test]
    async fn attaches_one_request_handle_to_the_events_and_tool_context_it_hands_out() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec![ContentBlock::ToolUse(ToolUseBlock::new(
            "probe",
            "t1",
            json!({}),
        ))]);
        model.add_turn(vec!["done".into()]);
        let seen: Arc<Mutex<Vec<InvocationContext>>> = Arc::new(Mutex::new(Vec::new()));
        let from_tool = Arc::clone(&seen);
        let probe = mock_tool("probe", move |ctx| {
            from_tool
                .lock()
                .unwrap()
                .push(ctx.invocation_context().unwrap().clone());
            text_result(ctx, "ok")
        });
        let mut agent = agent(&model, crate::tool_list![probe]);
        let from_hook = Arc::clone(&seen);
        agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
            from_hook
                .lock()
                .unwrap()
                .push(event.invocation_context().unwrap().clone());
        });
        let result = agent.invoke("go").await.unwrap();
        let contexts = seen.lock().unwrap();
        assert_eq!(contexts.len(), 3);
        assert!(
            contexts
                .iter()
                .all(|context| context.ptr_eq(result.invocation_context().unwrap()))
        );
    }

    #[tokio::test]
    async fn starts_fresh_request_state_on_each_root_invoke_of_a_reused_agent() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, Vec::new());
        let first = agent.invoke("a").await.unwrap();
        let second = agent.invoke("b").await.unwrap();
        assert!(
            !first
                .invocation_context()
                .unwrap()
                .ptr_eq(second.invocation_context().unwrap())
        );
        assert_eq!(second.invocation_context().unwrap().usage().turns_taken, 1);
    }

    #[tokio::test]
    async fn ends_on_a_limit_stop_even_when_a_hook_asks_to_resume() {
        let model = Arc::new(MockModel::new());
        let mut agent = looping(&model);
        agent.add_hook(|event: &mut AfterInvocationEvent<'_>| event.set_resume("again"));
        let options = InvokeOptions {
            limits: limits(Some(1), None),
            ..Default::default()
        };
        let result = agent.invoke_with("go", options).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::LimitTurns);
        assert_eq!(model.requests().len(), 1);
    }
}

mod state {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct Counter(u32);

    #[tokio::test]
    async fn returns_an_empty_object_on_agent_result_when_no_invocation_state_is_passed() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["ok".into()]);
        let result = agent(&model, Vec::new()).invoke("Hi").await.unwrap();
        assert!(result.invocation_state().is_empty());
    }

    #[tokio::test]
    async fn preserves_reference_identity_caller_keeps_the_same_object_they_passed_in() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["ok".into()]);
        let state = InvocationState::new();
        state.insert(Counter(1));
        let options = InvokeOptions {
            invocation_state: Some(state.clone()),
            ..Default::default()
        };
        let result = agent(&model, Vec::new())
            .invoke_with("Hi", options)
            .await
            .unwrap();
        assert!(result.invocation_state().ptr_eq(&state));
        assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(1)));
    }

    #[tokio::test]
    async fn propagates_mutations_from_before_model_call_event_to_after_model_call_event_and_agent_result() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
            event.invocation_state().insert(Counter(7));
        });
        let seen = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&seen);
        agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
            *sink.lock().unwrap() = event.invocation_state().get::<Counter>();
        });
        let result = agent.invoke("Hi").await.unwrap();
        assert_eq!(*seen.lock().unwrap(), Some(Counter(7)));
        assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(7)));
    }

    #[tokio::test]
    async fn passes_invocation_state_to_tools_via_tool_context_and_surfaces_mutations_on_the_result() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec![ContentBlock::ToolUse(ToolUseBlock::new(
            "bump",
            "t1",
            json!({}),
        ))]);
        model.add_turn(vec!["done".into()]);
        let bump = mock_tool("bump", |ctx| {
            let next = ctx
                .invocation_state()
                .get::<Counter>()
                .map_or(1, |counter| counter.0 + 1);
            ctx.invocation_state().insert(Counter(next));
            text_result(ctx, "bumped")
        });
        let mut agent = agent(&model, crate::tool_list![bump]);
        let result = agent.invoke("go").await.unwrap();
        assert_eq!(result.invocation_state().get::<Counter>(), Some(Counter(1)));
        assert!(agent.app_state().keys().is_empty());
    }

    #[tokio::test]
    async fn does_not_leak_state_between_invocations_on_the_same_agent_default_bag() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
            assert!(event.invocation_state().get::<Counter>().is_none());
            event.invocation_state().insert(Counter(1));
        });
        agent.invoke("a").await.unwrap();
        agent.invoke("b").await.unwrap();
    }

    #[tokio::test]
    async fn preserves_same_invocation_state_reference_across_after_model_call_event_retry() {
        let model = Arc::new(MockModel::new());
        model
            .add_error(|| crate::error::ModelError::new("x").into())
            .add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, Vec::new());
        let states: Arc<Mutex<Vec<InvocationState>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&states);
        agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
            sink.lock().unwrap().push(event.invocation_state().clone());
            if event.error().is_some() {
                event.set_retry(true);
            }
        });
        agent.invoke("Hi").await.unwrap();
        let states = states.lock().unwrap();
        assert_eq!(states.len(), 2);
        assert!(states[0].ptr_eq(&states[1]));
    }
}
