//! Ported from strands-ts/src/agent/__tests__/agent.hook.test.ts
//!
//! Plugin cases arrive with the plugin registry and the agent-cancel retry cases with cancel
//! semantics (Phase 4). The caller-owned array and shallow-copy cases are TS aliasing: `invoke`
//! and `set_end_turn` take ownership. In-place message mutation maps to `set_messages`.

use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::agent::{Agent, AgentConfig};
use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, BeforeInvocationEvent,
    BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, Cancel, EndTurn, MessageAddedEvent,
    ModelMessageEvent,
};
use crate::models::Model;
use crate::testing::{HookRecorder, MockModel, mock_tool, text_result};
use crate::tools::{ToolExecutor, ToolSource, ToolUse};
use crate::types::{ContentBlock, Message, Role, StopReason, ToolResultContent, ToolUseBlock};

fn agent(model: &Arc<MockModel>, tools: Vec<ToolSource>) -> Agent {
    Agent::new(AgentConfig {
        model: Some((Arc::clone(model) as Arc<dyn Model>).into()),
        tools,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn tool_turn(model: &MockModel, calls: &[(&str, &str)]) {
    model.add_turn(
        calls
            .iter()
            .map(|(name, id)| ContentBlock::ToolUse(ToolUseBlock::new(*name, *id, json!({ "x": 1 }))))
            .collect(),
    );
}

fn calc() -> ToolSource {
    ToolSource::from(mock_tool("calc", |ctx| text_result(ctx, "4")))
}

fn results(agent: &Agent) -> Vec<String> {
    agent
        .messages()
        .iter()
        .flat_map(|message| message.content.iter())
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => match result.content.first() {
                Some(ToolResultContent::Text(text)) => Some(text.text.clone()),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn texts(message: &Message) -> String {
    message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text.as_str()),
            _ => None,
        })
        .collect()
}

mod lifecycle {
    use super::*;

    #[tokio::test]
    async fn fires_hooks_during_invoke() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hi".into()]);
        let mut agent = agent(&model, Vec::new());
        let recorder = HookRecorder::attach(agent.hooks());
        agent.invoke("Hello").await.unwrap();
        let types = recorder.types();
        for expected in [
            "initializedEvent",
            "beforeInvocationEvent",
            "messageAddedEvent",
            "afterInvocationEvent",
            "agentResultEvent",
        ] {
            assert!(
                types.iter().any(|t| t == expected),
                "{expected} missing in {types:?}"
            );
        }
    }

    #[tokio::test]
    async fn fires_hooks_for_each_invoke_call() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hi".into()]);
        let mut agent = agent(&model, Vec::new());
        let recorder = HookRecorder::attach(agent.hooks());
        agent.invoke("one").await.unwrap();
        agent.invoke("two").await.unwrap();
        let types = recorder.types();
        assert_eq!(types.iter().filter(|t| *t == "initializedEvent").count(), 1);
        assert_eq!(types.iter().filter(|t| *t == "beforeInvocationEvent").count(), 2);
    }

    #[tokio::test]
    async fn uses_messages_replaced_by_a_hook() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hi".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
            event.set_messages(vec![Message::new(Role::User, vec!["replaced".into()])]);
        });
        agent.invoke("original").await.unwrap();
        assert_eq!(texts(&agent.messages()[0]), "replaced");
    }

    #[tokio::test]
    async fn uses_an_empty_replacement_from_a_hook() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hi".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_messages(Vec::new()));
        agent.invoke("original").await.unwrap();
        assert_eq!(agent.messages().len(), 1);
        assert_eq!(agent.messages()[0].role, Role::Assistant);
    }

    #[tokio::test]
    async fn fires_for_initial_user_input() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hi".into()]);
        let mut agent = agent(&model, Vec::new());
        let roles = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&roles);
        agent.add_hook(move |event: &mut MessageAddedEvent<'_>| {
            sink.lock().unwrap().push(event.message().role);
        });
        agent.invoke("Hello").await.unwrap();
        assert_eq!(*roles.lock().unwrap(), [Role::User, Role::Assistant]);
    }

    #[tokio::test]
    async fn fires_after_tool_call_event_with_error_when_tool_fails() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("bad", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let bad = crate::tools::FunctionTool::builder("bad", "Fails")
            .callback(|_, _| async { Err::<serde_json::Value, _>(crate::ToolError::msg("broken")) });
        let mut agent = agent(&model, crate::tool_list![bad]);
        let errors = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&errors);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            sink.lock()
                .unwrap()
                .push(event.error().map(|error| error.message.clone()));
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(*errors.lock().unwrap(), [Some("broken".to_owned())]);
    }
}

mod model_retry {
    use super::*;

    #[tokio::test]
    async fn does_not_duplicate_user_messages_on_error_retry() {
        let model = Arc::new(MockModel::new());
        model
            .add_error(|| crate::error::ModelError::new("x").into())
            .add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
            if event.error().is_some() {
                event.set_retry(true);
            }
        });
        agent.invoke("Hello").await.unwrap();
        assert_eq!(
            agent.messages().iter().map(|m| m.role).collect::<Vec<_>>(),
            [Role::User, Role::Assistant]
        );
    }

    #[tokio::test]
    async fn retries_model_call_on_success_when_hook_requests_it() {
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec!["first".into()])
            .add_turn(vec!["second".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
            if event.attempt_count() == 1 {
                event.set_retry(true);
            }
        });
        let result = agent.invoke("Hello").await.unwrap();
        assert_eq!(result.to_string(), "second");
        assert_eq!(agent.messages().len(), 2);
    }

    #[tokio::test]
    async fn does_not_retry_when_retry_is_not_set() {
        let model = Arc::new(MockModel::new());
        model
            .add_error(|| crate::error::ModelError::new("x").into())
            .add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, Vec::new());
        assert!(agent.invoke("Hello").await.is_err());
        assert_eq!(model.requests().len(), 1);
    }
}

mod tool_retry {
    use super::*;

    #[tokio::test]
    async fn retries_tool_call_when_hook_sets_retry_and_fires_before_tool_call_event_on_each_retry() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, vec![calc()]);
        let befores = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&befores);
        agent.add_hook(move |_: &mut BeforeToolCallEvent| *counter.lock().unwrap() += 1);
        let attempts = Arc::new(Mutex::new(0));
        let seen = Arc::clone(&attempts);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            let mut attempts = seen.lock().unwrap();
            *attempts += 1;
            if *attempts < 3 {
                event.set_retry(true);
            }
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(*attempts.lock().unwrap(), 3);
        assert_eq!(*befores.lock().unwrap(), 3);
        assert_eq!(results(&agent), ["4"]);
    }

    #[tokio::test]
    async fn allows_retry_after_cancel_on_before_tool_call_event() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, vec![calc()]);
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        agent.add_hook(move |event: &mut BeforeToolCallEvent| {
            let mut calls = counter.lock().unwrap();
            *calls += 1;
            if *calls == 1 {
                event.set_cancel("not yet");
            }
        });
        let retried = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&retried);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            let mut retried = flag.lock().unwrap();
            if !*retried {
                *retried = true;
                event.set_retry(true);
            }
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["4"]);
    }

    #[tokio::test]
    async fn allows_hooks_to_replace_result_on_after_tool_call_event_and_it_reaches_after_tools_event() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut AfterToolCallEvent| {
            let id = event.result().tool_use_id.clone();
            event.set_result(crate::tools::tool::error_result(&id, "replaced"));
        });
        let seen = Arc::new(Mutex::new(String::new()));
        let sink = Arc::clone(&seen);
        agent.add_hook(move |event: &mut AfterToolsEvent<'_>| {
            *sink.lock().unwrap() = serde_json::to_string(&event.message().content).unwrap();
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["replaced"]);
        assert!(seen.lock().unwrap().contains("replaced"));
    }
}

mod cancel_tools {
    use super::*;

    #[tokio::test]
    async fn cancels_individual_tool_call_with_default_message_when_cancel_is_true() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel(Cancel::Default));
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["Tool cancelled by hook"]);
    }

    #[tokio::test]
    async fn cancels_only_specific_tools_when_before_tool_call_event_selectively_cancels() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1"), ("other", "t2")]);
        model.add_turn(vec!["ok".into()]);
        let other = ToolSource::from(mock_tool("other", |ctx| text_result(ctx, "other ran")));
        let mut agent = agent(&model, vec![calc(), other]);
        agent.add_hook(|event: &mut BeforeToolCallEvent| {
            if event.tool_use().name == "calc" {
                event.set_cancel("no calc");
            }
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["no calc", "other ran"]);
    }

    #[tokio::test]
    async fn cancels_all_tools_with_default_message_when_before_tools_event_cancel_is_true() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1"), ("calc", "t2")]);
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(Cancel::Default));
        let recorder = HookRecorder::attach(agent.hooks());
        agent.invoke("go").await.unwrap();
        assert_eq!(
            results(&agent),
            ["Tool cancelled by hook", "Tool cancelled by hook"]
        );
        assert!(!recorder.types().iter().any(|t| t == "beforeToolCallEvent"));
        assert_eq!(
            recorder
                .types()
                .iter()
                .filter(|t| *t == "toolResultEvent")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn cancels_all_tools_with_custom_message_when_before_tools_event_cancel_is_a_string() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel("batch denied"));
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["batch denied"]);
    }
}

mod end_turn {
    use super::*;

    async fn ended(end_turn: Option<EndTurn>, executor: ToolExecutor) -> (Agent, crate::AgentResult) {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["model continued".into()]);
        let mut agent = agent(&model, vec![calc()]);
        agent.set_tool_executor(executor);
        agent.add_hook(move |event: &mut AfterToolsEvent<'_>| {
            if let Some(end_turn) = end_turn.clone() {
                event.set_end_turn(end_turn);
            }
        });
        let result = agent.invoke("go").await.unwrap();
        (agent, result)
    }

    #[tokio::test]
    async fn halts_the_loop_when_end_turn_is_true_with_default_message() {
        let (agent, result) = ended(Some(EndTurn::Default), ToolExecutor::Sequential).await;
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(
            result.to_string(),
            "Turn ended early by hook after tool execution"
        );
        assert_eq!(agent.messages().len(), 4);
    }

    #[tokio::test]
    async fn halts_the_loop_with_custom_assistant_message_when_end_turn_is_a_string() {
        let (_, result) = ended(Some(EndTurn::from("custom end")), ToolExecutor::Sequential).await;
        assert_eq!(result.to_string(), "custom end");
    }

    #[tokio::test]
    async fn does_not_halt_when_end_turn_is_false_default() {
        let (_, result) = ended(None, ToolExecutor::Sequential).await;
        assert_eq!(result.to_string(), "model continued");
    }

    #[tokio::test]
    async fn treats_empty_string_end_turn_as_falsy_does_not_halt() {
        let (_, result) = ended(Some(EndTurn::from("")), ToolExecutor::Sequential).await;
        assert_eq!(result.to_string(), "model continued");
    }

    #[tokio::test]
    async fn treats_empty_array_end_turn_as_falsy_does_not_halt() {
        let (_, result) = ended(Some(EndTurn::Content(Vec::new())), ToolExecutor::Sequential).await;
        assert_eq!(result.to_string(), "model continued");
    }

    #[tokio::test]
    async fn halts_with_content_blocks_when_end_turn_is_a_content_block_array() {
        let blocks = vec![ContentBlock::from("a"), ContentBlock::from("b")];
        let (agent, _) = ended(Some(EndTurn::Content(blocks.clone())), ToolExecutor::Sequential).await;
        assert_eq!(agent.messages().last().unwrap().content, blocks);
    }

    #[tokio::test]
    async fn halts_the_loop_with_concurrent_tool_execution() {
        let (_, result) = ended(Some(EndTurn::Default), ToolExecutor::Concurrent).await;
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
    }
}

mod cancel_invocation_and_model {
    use super::*;

    #[tokio::test]
    async fn cancels_invocation_with_default_message_and_does_not_append_user_message() {
        let model = Arc::new(MockModel::new());
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_cancel(Cancel::Default));
        let recorder = HookRecorder::attach(agent.hooks());
        let result = agent.invoke("go").await.unwrap();
        assert_eq!(result.to_string(), "invocation denied by hook");
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(
            agent.messages().iter().map(|m| m.role).collect::<Vec<_>>(),
            [Role::Assistant]
        );
        assert!(recorder.types().iter().any(|t| t == "afterInvocationEvent"));
        assert!(model.requests().is_empty());
    }

    #[tokio::test]
    async fn cancels_invocation_with_custom_message_when_cancel_is_a_string() {
        let mut agent = agent(&Arc::new(MockModel::new()), Vec::new());
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_cancel("closed"));
        assert_eq!(agent.invoke("go").await.unwrap().to_string(), "closed");
    }

    #[tokio::test]
    async fn cancels_model_call_and_emits_after_model_call_event_without_model_message_event() {
        let model = Arc::new(MockModel::new());
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.set_cancel(Cancel::Default));
        let messages = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&messages);
        agent.add_hook(move |_: &mut ModelMessageEvent<'_>| *counter.lock().unwrap() += 1);
        let recorder = HookRecorder::attach(agent.hooks());
        let result = agent.invoke("go").await.unwrap();
        assert_eq!(result.to_string(), "model call denied by hook");
        assert!(model.requests().is_empty());
        assert_eq!(*messages.lock().unwrap(), 0);
        assert!(recorder.types().iter().any(|t| t == "afterModelCallEvent"));
    }

    #[tokio::test]
    async fn allows_retry_after_cancel_on_model_call() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["real answer".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| {
            if event.agent().app_state().get_value("cancelled").is_none() {
                event.agent_mut().app_state().set("cancelled", true).unwrap();
                event.set_cancel("wait");
            }
        });
        agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
            if event.attempt_count() == 1 {
                event.set_retry(true);
            }
        });
        assert_eq!(agent.invoke("go").await.unwrap().to_string(), "real answer");
    }
}

mod tool_selection {
    use super::*;

    #[tokio::test]
    async fn cancel_wins_over_selected_tool() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, vec![calc()]);
        let replacement: Arc<dyn crate::tools::Tool> =
            Arc::new(mock_tool("other", |ctx| text_result(ctx, "other")));
        agent.add_hook(move |event: &mut BeforeToolCallEvent| {
            event.set_selected_tool(Arc::clone(&replacement));
            event.set_cancel("cancelled");
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["cancelled"]);
    }

    #[tokio::test]
    async fn passes_mutated_input_to_the_tool() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("echo", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let echo = ToolSource::from(mock_tool("echo", |ctx| {
            text_result(ctx, &ctx.tool_use().input.to_string())
        }));
        let mut agent = agent(&model, vec![echo]);
        agent.add_hook(|event: &mut BeforeToolCallEvent| {
            let mut tool_use = event.tool_use().clone();
            tool_use.input = json!({ "x": 2 });
            event.set_tool_use(tool_use);
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["{\"x\":2}"]);
    }

    #[tokio::test]
    async fn re_resolves_the_tool_when_hook_renames_tool_use_name() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let other = ToolSource::from(mock_tool("other", |ctx| text_result(ctx, "other ran")));
        let mut agent = agent(&model, vec![calc(), other]);
        agent.add_hook(|event: &mut BeforeToolCallEvent| {
            event.set_tool_use(ToolUse::new("other", "ignored-id", json!({})));
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["other ran"]);
        let ContentBlock::ToolResult(result) = &agent.messages()[2].content[0] else {
            panic!()
        };
        assert_eq!(result.tool_use_id, "t1");
    }
}

mod resume {
    use super::*;

    #[tokio::test]
    async fn re_invokes_the_agent_with_the_resume_args_and_emits_one_agent_result_event() {
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec!["first".into()])
            .add_turn(vec!["second".into()]);
        let mut agent = agent(&model, Vec::new());
        let resumed = Arc::new(Mutex::new(false));
        let flag = Arc::clone(&resumed);
        agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
            let mut resumed = flag.lock().unwrap();
            if !*resumed {
                *resumed = true;
                event.set_resume("follow up");
            }
        });
        let recorder = HookRecorder::attach(agent.hooks());
        let result = agent.invoke("go").await.unwrap();
        assert_eq!(result.to_string(), "second");
        assert_eq!(texts(&agent.messages()[2]), "follow up");
        assert_eq!(
            recorder
                .types()
                .iter()
                .filter(|t| *t == "agentResultEvent")
                .count(),
            1
        );
        assert_eq!(
            recorder
                .types()
                .iter()
                .filter(|t| *t == "beforeInvocationEvent")
                .count(),
            2
        );
    }

    #[tokio::test]
    async fn does_not_resume_when_the_invocation_errors() {
        let model = Arc::new(MockModel::new());
        model.add_error(|| crate::error::ModelError::new("boom").into());
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut AfterInvocationEvent<'_>| event.set_resume("again"));
        assert!(agent.invoke("go").await.is_err());
        assert_eq!(model.requests().len(), 1);
    }

    #[tokio::test]
    async fn first_registered_hook_wins_when_multiple_hooks_set_resume() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["a".into()]).add_turn(vec!["b".into()]);
        let mut agent = agent(&model, Vec::new());
        let count = Arc::new(Mutex::new(0));
        for text in ["first hook", "second hook"] {
            let count = Arc::clone(&count);
            agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
                if *count.lock().unwrap() < 2 {
                    *count.lock().unwrap() += 1;
                    event.set_resume(text);
                }
            });
        }
        agent.invoke("go").await.unwrap();
        assert_eq!(texts(&agent.messages()[2]), "first hook");
    }
}

mod more_cases {
    use futures::StreamExt;

    use super::*;
    use crate::agent::AgentStreamEvent;
    use crate::hooks::{BeforeInvocationEvent, ModelStreamUpdateEvent};
    use crate::tools::{Tool, ToolOutcome, tool::error_result};
    use crate::types::TextBlock;

    fn counter() -> (Arc<Mutex<usize>>, Arc<Mutex<usize>>) {
        let count = Arc::new(Mutex::new(0));
        (Arc::clone(&count), count)
    }

    #[tokio::test]
    async fn fires_hooks_during_stream() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hi".into()]);
        let mut agent = agent(&model, Vec::new());
        let recorder = HookRecorder::attach(agent.hooks());
        let streamed: Vec<String> = agent
            .stream("Hello")
            .map(|event| event.unwrap().event_type().to_owned())
            .collect()
            .await;
        let types = recorder.types();
        for expected in [
            "beforeInvocationEvent",
            "messageAddedEvent",
            "afterInvocationEvent",
        ] {
            assert!(
                types.iter().any(|t| t == expected),
                "{expected} missing in {types:?}"
            );
            assert!(
                streamed.iter().any(|t| t == expected),
                "{expected} missing in {streamed:?}"
            );
        }
    }

    #[tokio::test]
    async fn uses_messages_rewritten_by_a_hook() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
            let mut messages = event.messages().to_vec();
            messages[0].content[0] = TextBlock::new("redacted").into();
            event.set_messages(messages);
        });
        agent.invoke("secret").await.unwrap();
        assert_eq!(texts(&agent.messages()[0]), "redacted");
        assert_eq!(texts(&agent.messages()[1]), "Hello");
    }

    #[tokio::test]
    async fn fires_tool_hooks_during_tool_execution() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "tool-1")]);
        model.add_turn(vec!["Final response".into()]);
        let mut agent = agent(&model, vec![calc()]);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        agent.add_hook(move |event: &mut BeforeToolCallEvent| {
            sink.lock()
                .unwrap()
                .push(format!("before {}", event.tool_use().tool_use_id));
        });
        let sink = Arc::clone(&seen);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            sink.lock()
                .unwrap()
                .push(format!("after {}", event.result().tool_use_id));
        });
        let recorder = HookRecorder::attach(agent.hooks());
        agent.invoke("Test with tool").await.unwrap();
        assert_eq!(*seen.lock().unwrap(), ["before tool-1", "after tool-1"]);
        let added = recorder
            .types()
            .into_iter()
            .filter(|t| t == "messageAddedEvent")
            .count();
        assert_eq!(added, 4);
    }

    #[tokio::test]
    async fn model_stream_update_is_yielded_and_dispatched_to_hooks() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent(&model, Vec::new());
        let hooked = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&hooked);
        agent.add_hook(move |event: &mut ModelStreamUpdateEvent<'_>| {
            sink.lock()
                .unwrap()
                .push(serde_json::to_value(event.data()).unwrap());
        });
        let streamed: Vec<_> = agent
            .stream("Test")
            .filter_map(|event| async move {
                match event.unwrap() {
                    AgentStreamEvent::ModelStreamUpdate(data) => Some(serde_json::to_value(data).unwrap()),
                    _ => None,
                }
            })
            .collect()
            .await;
        assert!(!streamed.is_empty());
        assert_eq!(streamed, *hooked.lock().unwrap());
    }

    #[tokio::test]
    async fn does_not_duplicate_user_messages_on_success_retry() {
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec!["first".into()])
            .add_turn(vec!["second".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut AfterModelCallEvent<'_>| {
            if event.attempt_count() == 1 {
                event.set_retry(true);
            }
        });
        agent.invoke("Hello").await.unwrap();
        let roles: Vec<_> = agent.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant]);
        assert_eq!(model.requests()[1].messages.len(), 1);
    }

    #[tokio::test]
    async fn does_not_retry_tool_call_when_retry_is_not_set() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("fail", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let fail = mock_tool("fail", |ctx| {
            ToolOutcome::Result(error_result(&ctx.tool_use().tool_use_id, "Error: broken"))
        });
        let mut agent = agent(&model, vec![ToolSource::from(fail)]);
        let (seen, count) = counter();
        agent.add_hook(move |_: &mut AfterToolCallEvent| *seen.lock().unwrap() += 1);
        agent.invoke("go").await.unwrap();
        assert_eq!(*count.lock().unwrap(), 1);
        assert_eq!(results(&agent), ["Error: broken"]);
    }

    #[tokio::test]
    async fn cancels_individual_tool_call_with_custom_message() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel("Tool blocked by policy"));
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), ["Tool blocked by policy"]);
    }

    #[tokio::test]
    async fn cancels_all_tools_in_a_batch_with_their_tool_use_ids() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "tool-1"), ("calc", "tool-2")]);
        model.add_turn(vec!["Done".into()]);
        let (ran, executed) = counter();
        let tool = mock_tool("calc", move |ctx| {
            *ran.lock().unwrap() += 1;
            text_result(ctx, "4")
        });
        let mut agent = agent(&model, vec![ToolSource::from(tool)]);
        agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel("Batch cancelled"));
        let after = Arc::new(Mutex::new(None));
        let sink = Arc::clone(&after);
        agent.add_hook(move |event: &mut AfterToolsEvent<'_>| {
            *sink.lock().unwrap() = Some(serde_json::to_value(&event.message().content).unwrap());
        });
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*executed.lock().unwrap(), 0);
        let expected = json!([
            {"toolResult": {"toolUseId": "tool-1", "status": "error", "content": [{"text": "Batch cancelled"}]}},
            {"toolResult": {"toolUseId": "tool-2", "status": "error", "content": [{"text": "Batch cancelled"}]}},
        ]);
        assert_eq!(after.lock().unwrap().clone().unwrap(), expected);
    }

    #[tokio::test]
    async fn appends_tool_results_and_the_default_end_turn_message() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(EndTurn::Default));
        agent.invoke("Test").await.unwrap();
        let roles: Vec<_> = agent.messages().iter().map(|m| m.role).collect();
        assert_eq!(roles, [Role::User, Role::Assistant, Role::User, Role::Assistant]);
        assert!(matches!(agent.messages()[1].content[0], ContentBlock::ToolUse(_)));
        assert!(matches!(
            agent.messages()[2].content[0],
            ContentBlock::ToolResult(_)
        ));
        assert_eq!(
            texts(&agent.messages()[3]),
            "Turn ended early by hook after tool execution"
        );
    }

    #[tokio::test]
    async fn halts_even_when_end_turn_is_set_on_a_cancelled_tools_event() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        model.add_turn(vec!["never".into()]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut BeforeToolsEvent<'_>| event.set_cancel(Cancel::Default));
        agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(EndTurn::Default));
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().role, Role::Assistant);
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn emits_after_invocation_event_when_the_invocation_is_cancelled() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| event.set_cancel(Cancel::Default));
        let recorder = HookRecorder::attach(agent.hooks());
        agent.invoke("Test").await.unwrap();
        let types = recorder.types();
        assert_eq!(types.iter().filter(|t| *t == "beforeInvocationEvent").count(), 1);
        assert_eq!(types.iter().filter(|t| *t == "afterInvocationEvent").count(), 1);
    }

    #[tokio::test]
    async fn cancels_model_call_with_custom_message() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut BeforeModelCallEvent<'_>| event.set_cancel("Rate limited"));
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(texts(result.last_message()), "Rate limited");
    }

    #[tokio::test]
    async fn invokes_the_selected_tool_instead_of_the_registry_tool_with_either_executor() {
        for executor in [ToolExecutor::Sequential, ToolExecutor::Concurrent] {
            let model = Arc::new(MockModel::new());
            tool_turn(&model, &[("originalTool", "tool-1")]);
            model.add_turn(vec!["Done".into()]);
            let (ran, original_runs) = counter();
            let original = mock_tool("originalTool", move |ctx| {
                *ran.lock().unwrap() += 1;
                text_result(ctx, "original")
            });
            let replacement: Arc<dyn Tool> = Arc::new(mock_tool("replacementTool", |ctx| {
                text_result(ctx, "replacement")
            }));
            let mut agent = agent(&model, vec![ToolSource::from(original)]);
            agent.set_tool_executor(executor);
            agent.add_hook(move |event: &mut BeforeToolCallEvent| {
                event.set_selected_tool(Arc::clone(&replacement));
            });
            agent.invoke("Test").await.unwrap();
            assert_eq!(*original_runs.lock().unwrap(), 0);
            assert_eq!(results(&agent), ["replacement"]);
        }
    }

    #[tokio::test]
    async fn passes_mutated_input_to_the_tool_with_the_sequential_executor() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("echo", "t1")]);
        model.add_turn(vec!["ok".into()]);
        let echo = mock_tool("echo", |ctx| text_result(ctx, &ctx.tool_use().input.to_string()));
        let mut agent = agent(&model, vec![ToolSource::from(echo)]);
        agent.set_tool_executor(ToolExecutor::Sequential);
        agent.add_hook(|event: &mut BeforeToolCallEvent| {
            let tool_use = ToolUse {
                input: json!({ "x": 2 }),
                ..event.tool_use().clone()
            };
            event.set_tool_use(tool_use);
        });
        agent.invoke("go").await.unwrap();
        assert_eq!(results(&agent), [r#"{"x":2}"#]);
    }

    async fn streamed(agent: &mut Agent) -> Vec<AgentStreamEvent> {
        agent.stream("Test").map(Result::unwrap).collect().await
    }

    #[tokio::test]
    async fn emits_cancel_events_via_stream() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "tool-1")]);
        model.add_turn(vec!["Done".into()]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel("Cancelled via stream"));
        let items = streamed(&mut agent).await;
        let count = |kind: &str| items.iter().filter(|e| e.event_type() == kind).count();
        assert_eq!(count("beforeToolCallEvent"), 1);
        let after = items
            .iter()
            .find(|e| e.event_type() == "afterToolCallEvent")
            .map(|e| serde_json::to_value(e).unwrap())
            .unwrap();
        assert_eq!(
            after["result"],
            json!({"toolResult": {"toolUseId": "tool-1", "status": "error", "content": [{"text": "Cancelled via stream"}]}})
        );
    }

    #[tokio::test]
    async fn emits_after_tools_event_and_one_result_with_end_turn_via_stream() {
        let model = Arc::new(MockModel::new());
        tool_turn(&model, &[("calc", "t1")]);
        let mut agent = agent(&model, vec![calc()]);
        agent.add_hook(|event: &mut AfterToolsEvent<'_>| event.set_end_turn(EndTurn::Default));
        let items = streamed(&mut agent).await;
        assert_eq!(
            items
                .iter()
                .filter(|e| e.event_type() == "afterToolsEvent")
                .count(),
            1
        );
        let results: Vec<_> = items
            .iter()
            .filter_map(|e| match e {
                AgentStreamEvent::AgentResult(result) => Some(result.stop_reason()),
                _ => None,
            })
            .collect();
        assert_eq!(results, [StopReason::EndTurn]);
    }

    #[tokio::test]
    async fn chains_multiple_resumes() {
        let model = Arc::new(MockModel::new());
        model
            .add_turn(vec!["a".into()])
            .add_turn(vec!["b".into()])
            .add_turn(vec!["c".into()]);
        let mut agent = agent(&model, Vec::new());
        let (seen, count) = counter();
        agent.add_hook(move |event: &mut AfterInvocationEvent<'_>| {
            let mut count = seen.lock().unwrap();
            *count += 1;
            match *count {
                1 => event.set_resume("second"),
                2 => event.set_resume("third"),
                _ => {}
            }
        });
        let result = agent.invoke("first").await.unwrap();
        assert_eq!(*count.lock().unwrap(), 3);
        assert_eq!(texts(result.last_message()), "c");
    }

    #[tokio::test]
    async fn does_not_resume_when_resume_is_left_unset() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["only".into()]);
        let mut agent = agent(&model, Vec::new());
        let (seen, count) = counter();
        agent.add_hook(move |_: &mut AfterInvocationEvent<'_>| *seen.lock().unwrap() += 1);
        agent.invoke("hi").await.unwrap();
        assert_eq!(*count.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn ignores_resume_set_during_an_erroring_invocation() {
        let model = Arc::new(MockModel::new());
        model.add_error(|| crate::error::ModelError::new("boom").into());
        let mut agent = agent(&model, Vec::new());
        agent.add_hook(|event: &mut AfterInvocationEvent<'_>| event.set_resume("should not run"));
        let (seen, befores) = counter();
        agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| *seen.lock().unwrap() += 1);
        let error = agent.invoke("hi").await.unwrap_err();
        assert!(error.to_string().contains("boom"), "{error}");
        assert_eq!(*befores.lock().unwrap(), 1);
    }
}
