//! Interrupts and resume, ported from strands-ts/src/agent/__tests__/agent.interrupt.test.ts.

use super::*;
use crate::hooks::{AfterToolCallEvent, BeforeToolCallEvent, InterruptEvent};
use crate::interrupt::{InterruptParams, InterruptResponse, InterruptSource};
use crate::tool_list;
use crate::tools::ToolContext;

fn respond(result: &AgentResult, response: &JsonValue) -> Vec<InterruptResponse> {
    result
        .interrupts()
        .iter()
        .map(|interrupt| InterruptResponse::new(interrupt.id(), response.clone()))
        .collect()
}

fn approved(response: &JsonValue) -> bool {
    response["approved"] == json!(true)
}

/// A tool that asks `name` and logs its label once the answer arrives.
fn asking_tool(tool: &str, name: &'static str, log: &Arc<Mutex<Vec<String>>>) -> FunctionTool {
    let log = Arc::clone(log);
    let label = tool.to_owned();
    FunctionTool::new(tool, "asks", move |_input: JsonValue, ctx: ToolContext| {
        let log = Arc::clone(&log);
        let label = label.clone();
        async move {
            let response = ctx.interrupt(InterruptParams {
                name: name.into(),
                reason: Some(json!("Confirm?")),
                ..Default::default()
            })?;
            log.lock().unwrap().push(label);
            Ok::<_, ToolError>(if approved(&response) { "approved" } else { "denied" })
        }
    })
}

fn logging_tool(tool: &str, log: &Arc<Mutex<Vec<String>>>) -> FunctionTool {
    let log = Arc::clone(log);
    let label = tool.to_owned();
    FunctionTool::new(tool, "logs", move |_input: JsonValue, _ctx| {
        let log = Arc::clone(&log);
        let label = label.clone();
        async move {
            log.lock().unwrap().push(label.clone());
            Ok::<_, ToolError>(format!("{label} result"))
        }
    })
}

fn tool_result_texts(message: &Message) -> Vec<String> {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(serde_json::to_value(result).unwrap()),
            _ => None,
        })
        .map(|value| {
            value["content"][0]["text"]
                .as_str()
                .unwrap_or_default()
                .to_owned()
        })
        .collect()
}

fn tool_result_message(agent: &Agent) -> &Message {
    agent
        .messages()
        .iter()
        .find(|message| {
            message
                .content()
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult(_)))
        })
        .unwrap()
}

fn approve_hook(agent: &mut Agent, tool: &'static str) {
    agent.add_hook(move |event: &mut BeforeToolCallEvent| {
        if event.tool_use().name() == tool {
            let approval = event.interrupt(InterruptParams {
                name: "approve_delete".into(),
                reason: Some(json!("Confirm delete?")),
                ..Default::default()
            });
            if let Some(approval) = approval
                && approval != json!("yes")
            {
                event.set_cancel("not approved");
            }
        }
    });
}

#[tokio::test]
async fn returns_stop_reason_interrupt_when_tool_calls_interrupt() {
    let log = Arc::new(Mutex::new(Vec::new()));
    let model = mock(vec![vec![tool_use("confirmTool", "tool-1", json!({}))]]);
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .tools([asking_tool("confirmTool", "confirm", &log)])
        .build()
        .unwrap();
    let result = agent.invoke("Do it").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::Interrupt);
    let [interrupt] = result.interrupts() else {
        panic!("one interrupt")
    };
    assert_eq!(interrupt.id(), "tool:tool-1:confirm");
    assert_eq!(interrupt.name(), "confirm");
    assert_eq!(interrupt.reason(), Some(&json!("Confirm?")));
    assert_eq!(interrupt.source(), &InterruptSource::Tool);
    assert!(log.lock().unwrap().is_empty());
}

mod before_tool_call_hook {
    use super::*;

    #[tokio::test]
    async fn stores_pending_state_and_resumes_correctly_after_interrupt() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let model = mock(vec![
            vec![tool_use("deleteTool", "tool-1", json!({"key": "X"}))],
            vec![text("Deleted")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([logging_tool("deleteTool", &log)])
            .build()
            .unwrap();
        approve_hook(&mut agent, "deleteTool");

        let interrupted = agent.invoke("Delete X").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        let [interrupt] = interrupted.interrupts() else {
            panic!("one interrupt")
        };
        assert_eq!(interrupt.id(), "hook:beforeToolCall:tool-1:approve_delete");
        assert_eq!(interrupt.source(), &InterruptSource::Hook);
        assert!(log.lock().unwrap().is_empty());
        assert_eq!(model.call_count(), 1);
        {
            let state = agent.local.interrupt_state().lock();
            let pending = state.pending_tool_execution.as_ref().unwrap();
            assert_eq!(
                serde_json::to_value(pending.assistant_message_data.content()).unwrap(),
                json!([{"toolUse": {"name": "deleteTool", "toolUseId": "tool-1", "input": {"key": "X"}}}])
            );
            assert!(pending.completed_tool_results.is_empty());
        }

        let result = agent.invoke(respond(&interrupted, &json!("yes"))).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*log.lock().unwrap(), ["deleteTool"]);
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn preserves_completed_tool_results_when_interrupt_fires_on_a_later_tool() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let model = mock(vec![
            vec![
                tool_use("readTool", "tool-1", json!({})),
                tool_use("deleteTool", "tool-2", json!({})),
            ],
            vec![text("Done")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([logging_tool("readTool", &log), logging_tool("deleteTool", &log)])
            .tool_executor(SequentialToolExecutor::new())
            .build()
            .unwrap();
        approve_hook(&mut agent, "deleteTool");

        let interrupted = agent.invoke("Go").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        {
            let state = agent.local.interrupt_state().lock();
            let pending = state.pending_tool_execution.as_ref().unwrap();
            assert_eq!(
                pending.completed_tool_results.keys().collect::<Vec<_>>(),
                ["tool-1"]
            );
        }
        agent.invoke(respond(&interrupted, &json!("yes"))).await.unwrap();
        assert_eq!(*log.lock().unwrap(), ["readTool", "deleteTool"]);
        assert_eq!(
            tool_result_texts(tool_result_message(&agent)),
            ["readTool result", "deleteTool result"]
        );
    }

    #[tokio::test]
    async fn a_denied_approval_cancels_the_tool() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let model = mock(vec![
            vec![tool_use("deleteTool", "tool-1", json!({}))],
            vec![text("Not deleted")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([logging_tool("deleteTool", &log)])
            .build()
            .unwrap();
        approve_hook(&mut agent, "deleteTool");
        let interrupted = agent.invoke("Delete").await.unwrap();
        let result = agent.invoke(respond(&interrupted, &json!("no"))).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert!(log.lock().unwrap().is_empty());
        assert_eq!(tool_result_texts(tool_result_message(&agent)), ["not approved"]);
    }
}

mod before_tools_hook {
    use super::*;

    #[tokio::test]
    async fn returns_stop_reason_interrupt_when_hook_calls_interrupt() {
        let model = mock(vec![
            vec![tool_use("add", "tool-1", json!({"a": 1, "b": 2}))],
            vec![text("Done")],
        ]);
        let mut agent = tool_agent(&model);
        agent.add_hook(|event: &mut BeforeToolsEvent<'_>| {
            event.interrupt(InterruptParams::new("approve_batch"));
        });
        let result = agent.invoke("Go").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        let [interrupt] = result.interrupts() else {
            panic!("one interrupt")
        };
        assert_eq!(interrupt.name(), "approve_batch");
        assert_eq!(interrupt.source(), &InterruptSource::Hook);
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn interrupts_again_on_cycle_2_after_resuming_from_cycle_1() {
        let model = mock(vec![
            vec![tool_use("add", "tool-1", json!({"a": 1, "b": 2}))],
            vec![tool_use("add", "tool-2", json!({"a": 3, "b": 4}))],
            vec![text("Done")],
        ]);
        let mut agent = tool_agent(&model);
        let count = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&count);
        agent.add_hook(move |event: &mut BeforeToolsEvent<'_>| {
            *counter.lock().unwrap() += 1;
            event.interrupt(InterruptParams {
                name: "approval".into(),
                reason: Some(json!("Approve?")),
                ..Default::default()
            });
        });
        let first = agent.invoke("Go").await.unwrap();
        assert_eq!(first.stop_reason(), StopReason::Interrupt);
        assert_eq!(first.interrupts()[0].reason(), Some(&json!("Approve?")));
        assert_eq!(*count.lock().unwrap(), 1);
        let second = agent.invoke(respond(&first, &json!("yes"))).await.unwrap();
        assert_eq!(second.stop_reason(), StopReason::Interrupt);
        assert_eq!(*count.lock().unwrap(), 3);
        let result = agent.invoke(respond(&second, &json!("yes"))).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(model.call_count(), 3);
    }
}

mod resume {
    use super::*;

    #[tokio::test]
    async fn resumes_tool_callback_execution_without_re_calling_model() {
        let calls = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&calls);
        let tool = FunctionTool::new(
            "confirmTool",
            "confirms",
            move |input: JsonValue, ctx: ToolContext| {
                let seen = Arc::clone(&seen);
                async move {
                    seen.lock().unwrap().push(None);
                    let amount = &input["amount"];
                    let response = ctx.interrupt(InterruptParams {
                        name: "confirm_transfer".into(),
                        reason: Some(json!(format!("Confirm transfer of ${amount}?"))),
                        ..Default::default()
                    })?;
                    *seen.lock().unwrap().last_mut().unwrap() = Some(response.clone());
                    Ok::<_, ToolError>(if approved(&response) {
                        "Transfer approved"
                    } else {
                        "Transfer denied"
                    })
                }
            },
        );
        let model = mock(vec![
            vec![tool_use("confirmTool", "tool-1", json!({"amount": 5000}))],
            vec![text("Transfer completed")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([tool])
            .build()
            .unwrap();

        let interrupted = agent.invoke("Transfer $5000").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        assert_eq!(interrupted.interrupts()[0].name(), "confirm_transfer");
        assert_eq!(
            interrupted.interrupts()[0].reason(),
            Some(&json!("Confirm transfer of $5000?"))
        );
        assert_eq!(calls.lock().unwrap().len(), 1);
        assert_eq!(model.call_count(), 1);

        let result = agent
            .invoke(respond(&interrupted, &json!({"approved": true})))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*calls.lock().unwrap(), [None, Some(json!({"approved": true}))]);
        assert_eq!(model.call_count(), 2);
        assert_eq!(
            tool_result_texts(tool_result_message(&agent)),
            ["Transfer approved"]
        );
        let state = agent.local.interrupt_state().lock();
        assert!(!state.activated && state.interrupts.is_empty());
        assert!(state.pending_tool_execution.is_none());
    }

    #[tokio::test]
    async fn does_not_append_hook_messages_when_resuming_an_interrupted_tool() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let model = mock(vec![
            vec![tool_use("confirmTool", "tool-1", json!({}))],
            vec![text("Done")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([asking_tool("confirmTool", "confirm", &log)])
            .build()
            .unwrap();
        agent.add_hook(|event: &mut BeforeInvocationEvent<'_>| {
            event
                .messages_mut()
                .push(Message::new(Role::User, vec![text("hook context")]));
        });
        let interrupted = agent.invoke("Go").await.unwrap();
        let before = agent.messages().len();
        agent
            .invoke(respond(&interrupted, &json!({"approved": true})))
            .await
            .unwrap();
        let added = &agent.messages()[before..];
        assert_eq!(
            added.iter().map(Message::role).collect::<Vec<_>>(),
            [Role::Assistant, Role::User, Role::Assistant]
        );
        assert!(added.iter().all(|message| texts(message) != ["hook context"]));
    }

    #[tokio::test]
    async fn skips_already_completed_tools_when_resuming_from_partial_execution() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let model = mock(vec![
            vec![
                tool_use("toolA", "tool-a", json!({})),
                tool_use("toolB", "tool-b", json!({})),
                tool_use("toolC", "tool-c", json!({})),
            ],
            vec![text("All tools completed")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools(tool_list![
                logging_tool("toolA", &log),
                logging_tool("toolB", &log),
                asking_tool("toolC", "confirm_c", &log)
            ])
            .build()
            .unwrap();
        let interrupted = agent.invoke("Run all tools").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        assert_eq!(interrupted.interrupts()[0].name(), "confirm_c");
        assert_eq!(*log.lock().unwrap(), ["toolA", "toolB"]);

        let result = agent
            .invoke(respond(&interrupted, &json!({"approved": true})))
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*log.lock().unwrap(), ["toolA", "toolB", "toolC"]);
        assert_eq!(model.call_count(), 2);
        assert_eq!(tool_result_texts(tool_result_message(&agent)).len(), 3);
    }

    #[tokio::test]
    async fn throws_when_sending_a_new_message_while_in_interrupted_state() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let model = mock(vec![vec![tool_use("confirmTool", "tool-1", json!({}))]]);
        let mut agent = Agent::builder()
            .model(model)
            .tools([asking_tool("confirmTool", "confirm", &log)])
            .build()
            .unwrap();
        agent.invoke("Go").await.unwrap();
        let error = agent.invoke("New message").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Agent is in an interrupted state. Resume by invoking with interruptResponse content blocks."
        );
    }

    #[tokio::test]
    async fn rejects_a_response_for_an_unknown_interrupt() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let model = mock(vec![vec![tool_use("confirmTool", "tool-1", json!({}))]]);
        let mut agent = Agent::builder()
            .model(model)
            .tools([asking_tool("confirmTool", "confirm", &log)])
            .build()
            .unwrap();
        agent.invoke("Go").await.unwrap();
        let error = agent
            .invoke(vec![InterruptResponse::new("nope", json!(true))])
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "interrupt_id=<nope> | no interrupt found");
    }

    #[tokio::test]
    async fn resumes_correctly_after_multiple_interrupts_are_answered() {
        let model = mock(vec![
            vec![tool_use("add", "tool-1", json!({"a": 1, "b": 2}))],
            vec![text("Done")],
        ]);
        let mut agent = tool_agent(&model);
        for name in ["first", "second"] {
            agent.add_hook(move |event: &mut BeforeToolCallEvent| {
                event.interrupt(InterruptParams::new(name));
            });
        }
        let interrupted = agent.invoke("Go").await.unwrap();
        assert_eq!(
            interrupted
                .interrupts()
                .iter()
                .map(crate::interrupt::Interrupt::name)
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        let result = agent.invoke(respond(&interrupted, &json!("ok"))).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(tool_result_texts(tool_result_message(&agent)), ["3"]);
    }
}

mod event_contract {
    use super::*;

    #[tokio::test]
    async fn does_not_fire_after_tool_call_event_when_a_hook_or_tool_interrupts() {
        let log = Arc::new(Mutex::new(Vec::new()));
        for hook in [true, false] {
            let model = mock(vec![vec![tool_use("confirmTool", "tool-1", json!({}))]]);
            let mut agent = Agent::builder()
                .model(model)
                .tools([asking_tool("confirmTool", "confirm", &log)])
                .build()
                .unwrap();
            if hook {
                approve_hook(&mut agent, "confirmTool");
            }
            let fired = Arc::new(Mutex::new(0));
            let count = Arc::clone(&fired);
            agent.add_hook(move |_: &mut AfterToolCallEvent| *count.lock().unwrap() += 1);
            let result = agent.invoke("Go").await.unwrap();
            assert_eq!(result.stop_reason(), StopReason::Interrupt);
            assert_eq!(*fired.lock().unwrap(), 0);
        }
    }

    #[tokio::test]
    async fn yields_one_interrupt_event_per_interrupt_tagged_with_source() {
        let model = mock(vec![vec![tool_use("add", "tool-1", json!({"a": 1, "b": 2}))]]);
        let mut agent = tool_agent(&model);
        for name in ["first", "second"] {
            agent.add_hook(move |event: &mut BeforeToolCallEvent| {
                event.interrupt(InterruptParams::new(name));
            });
        }
        let hooked = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&hooked);
        agent.add_hook(move |event: &mut InterruptEvent<'_>| {
            seen.lock().unwrap().push(event.interrupt().name().to_owned());
        });
        let events = collect(agent.stream("Go")).await;
        let streamed: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentStreamEvent::Interrupt(data) => Some(serde_json::to_value(data).unwrap()),
                _ => None,
            })
            .collect();
        assert_eq!(streamed.len(), 2);
        assert_eq!(streamed[0]["interrupt"]["source"], json!("hook"));
        assert_eq!(*hooked.lock().unwrap(), ["first", "second"]);
    }
}

mod concurrent {
    use super::*;

    #[tokio::test]
    async fn allows_in_flight_tool_to_complete_when_sibling_interrupts() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::sync::Notify;

        let model = mock(vec![
            vec![
                tool_use("toolA", "tool-a", json!({})),
                tool_use("toolB", "tool-b", json!({})),
            ],
            vec![text("Done")],
        ]);
        let started = Arc::new(Notify::new());
        let gate = Arc::new(Notify::new());
        let completed = Arc::new(AtomicBool::new(false));
        let (a_started, a_gate, a_completed) =
            (Arc::clone(&started), Arc::clone(&gate), Arc::clone(&completed));
        let tool_a = FunctionTool::new("toolA", "gated", move |_input: JsonValue, _ctx| {
            let (started, gate, completed) = (
                Arc::clone(&a_started),
                Arc::clone(&a_gate),
                Arc::clone(&a_completed),
            );
            async move {
                started.notify_one();
                gate.notified().await;
                completed.store(true, Ordering::SeqCst);
                Ok::<_, ToolError>("A done")
            }
        });
        let tool_b = FunctionTool::new(
            "toolB",
            "asks",
            |_input: JsonValue, ctx: ToolContext| async move {
                ctx.interrupt(InterruptParams {
                    name: "confirm_b".into(),
                    reason: Some(json!("Approve B?")),
                    ..Default::default()
                })?;
                Ok::<_, ToolError>("B done")
            },
        );
        let mut agent = Agent::builder()
            .model(model)
            .tools(tool_list![tool_a, tool_b])
            .build()
            .unwrap();
        let release = async {
            started.notified().await;
            assert!(!completed.load(Ordering::SeqCst));
            gate.notify_one();
        };
        let (result, ()) = tokio::join!(agent.invoke("Go"), release);
        let result = result.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        assert!(completed.load(Ordering::SeqCst));
        let [interrupt] = result.interrupts() else {
            panic!("one interrupt")
        };
        assert_eq!(interrupt.name(), "confirm_b");
        assert_eq!(interrupt.source(), &InterruptSource::Tool);
        let state = agent.local.interrupt_state().lock();
        let pending = state.pending_tool_execution.as_ref().unwrap();
        assert_eq!(
            serde_json::to_value(&pending.completed_tool_results["tool-a"]).unwrap(),
            json!({"toolResult": {"toolUseId": "tool-a", "status": "success", "content": [{"text": "A done"}]}})
        );
    }

    #[tokio::test]
    async fn stores_completed_tool_results_and_resumes_only_the_interrupted_tool() {
        let log = Arc::new(Mutex::new(Vec::new()));
        let model = mock(vec![
            vec![
                tool_use("fast", "tool-1", json!({})),
                tool_use("confirmTool", "tool-2", json!({})),
            ],
            vec![text("Done")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools(tool_list![
                logging_tool("fast", &log),
                asking_tool("confirmTool", "confirm", &log)
            ])
            .build()
            .unwrap();
        let interrupted = agent.invoke("Go").await.unwrap();
        assert_eq!(*log.lock().unwrap(), ["fast"]);
        agent
            .invoke(respond(&interrupted, &json!({"approved": true})))
            .await
            .unwrap();
        assert_eq!(*log.lock().unwrap(), ["fast", "confirmTool"]);
        assert_eq!(model.call_count(), 2);
        let mut results = tool_result_texts(tool_result_message(&agent));
        results.sort();
        assert_eq!(results, ["approved", "fast result"]);
    }
}

mod hook_rewritten_tool_use_id {
    use super::*;
    use crate::tools::{ConcurrentToolExecutor, ToolExecutor};
    use crate::types::tools::ToolUse;

    /// The tool use and tool result ids of every request that carried tool results.
    fn provider_ids(model: &MockMessageModel) -> Vec<(Vec<String>, Vec<String>)> {
        model
            .requests()
            .iter()
            .map(|request| {
                let content: Vec<&ContentBlock> =
                    request.messages.iter().flat_map(Message::content).collect();
                let uses = content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolUse(tool_use) => Some(tool_use.tool_use_id().to_owned()),
                        _ => None,
                    })
                    .collect();
                let results: Vec<String> = content
                    .iter()
                    .filter_map(|block| match block {
                        ContentBlock::ToolResult(result) => Some(result.tool_use_id().to_owned()),
                        _ => None,
                    })
                    .collect();
                (uses, results)
            })
            .filter(|(_, results)| !results.is_empty())
            .collect()
    }

    #[tokio::test]
    async fn runs_the_completed_tool_exactly_once_across_interrupt_and_resume() {
        for executor in [
            ToolExecutor::Sequential(SequentialToolExecutor::new()),
            ToolExecutor::Concurrent(ConcurrentToolExecutor::new()),
        ] {
            let log = Arc::new(Mutex::new(Vec::new()));
            let model = mock(vec![
                vec![
                    tool_use("toolA", "tool-a", json!({})),
                    tool_use("toolB", "tool-b", json!({})),
                ],
                vec![text("Done")],
            ]);
            let tool_b_log = Arc::clone(&log);
            let tool_b = FunctionTool::new("toolB", "asks", move |_input: JsonValue, ctx: ToolContext| {
                let log = Arc::clone(&tool_b_log);
                async move {
                    log.lock().unwrap().push("toolB".to_owned());
                    let response = ctx.interrupt(InterruptParams::new("confirm_b"))?;
                    Ok::<_, ToolError>(format!("B: {response}"))
                }
            });
            let mut agent = Agent::builder()
                .model(Arc::clone(&model))
                .tools(tool_list![logging_tool("toolA", &log), tool_b])
                .tool_executor(executor)
                .build()
                .unwrap();
            agent.add_hook(|event: &mut BeforeToolCallEvent| {
                if event.tool_use().name() == "toolA" {
                    let tool_use = event.tool_use();
                    let rewritten = ToolUse::new(tool_use.name(), "rewritten-a", tool_use.input().clone());
                    event.set_tool_use(rewritten);
                }
            });
            agent.add_hook(|event: &mut AfterToolCallEvent| {
                if event.tool_use().name() == "toolA" {
                    let result = event.result();
                    let rewritten = ToolResultBlock::new(
                        "after-rewritten-a",
                        result.status().clone(),
                        result.content().to_vec(),
                    );
                    event.set_result(rewritten);
                }
            });

            let interrupted = agent.invoke("Go").await.unwrap();
            assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
            assert_eq!(*log.lock().unwrap(), ["toolA", "toolB"]);
            {
                let state = agent.local.interrupt_state().lock();
                let pending = state.pending_tool_execution.as_ref().unwrap();
                assert_eq!(
                    pending.completed_tool_results.keys().collect::<Vec<_>>(),
                    ["tool-a"]
                );
                assert_eq!(
                    pending.completed_tool_results["tool-a"].tool_result.tool_use_id(),
                    "tool-a"
                );
            }

            let result = agent
                .invoke(respond(&interrupted, &json!("approved")))
                .await
                .unwrap();
            assert_eq!(result.stop_reason(), StopReason::EndTurn);
            assert_eq!(*log.lock().unwrap(), ["toolA", "toolB", "toolB"]);
            let ids = |values: &[&str]| values.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>();
            assert_eq!(
                provider_ids(&model),
                [(ids(&["tool-a", "tool-b"]), ids(&["tool-a", "tool-b"]))]
            );
        }
    }
}

mod cancel {
    use std::sync::atomic::{AtomicBool, Ordering};

    use tokio::sync::Notify;

    use super::*;
    use crate::cancel::CancelOptions;
    use crate::hooks::BeforeToolsEvent;

    fn has_pending(agent: &Agent) -> bool {
        agent
            .local
            .interrupt_state()
            .lock()
            .pending_tool_execution
            .is_some()
    }

    #[tokio::test]
    async fn preserves_the_pending_tool_interrupt_when_a_resume_is_cancelled_before_the_tool_runs() {
        let model = mock(vec![
            vec![tool_use("confirmTool", "tool-1", json!({}))],
            vec![text("Done")],
        ]);
        let executed = Arc::new(AtomicBool::new(false));
        let ran = Arc::clone(&executed);
        let tool = FunctionTool::new(
            "confirmTool",
            "asks",
            move |_input: JsonValue, ctx: ToolContext| {
                let ran = Arc::clone(&ran);
                async move {
                    if !ran.load(Ordering::SeqCst) {
                        ctx.interrupt(InterruptParams {
                            name: "confirm".into(),
                            reason: Some(json!("Approve?")),
                            ..Default::default()
                        })?;
                    }
                    ran.store(true, Ordering::SeqCst);
                    Ok::<_, ToolError>("executed")
                }
            },
        );
        let mut agent = Agent::builder().model(model).tools([tool]).build().unwrap();

        let interrupted = agent.invoke("Go").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        assert!(has_pending(&agent));

        let cancel_hook = agent.add_hook(|event: &mut BeforeToolsEvent<'_>| {
            event.agent().cancel(CancelOptions::default());
        });
        let cancelled = agent.invoke(respond(&interrupted, &json!("yes"))).await.unwrap();
        assert_eq!(cancelled.stop_reason(), StopReason::Cancelled);
        assert!(!executed.load(Ordering::SeqCst));
        assert!(has_pending(&agent));

        cancel_hook.remove();
        let finished = agent.invoke(respond(&interrupted, &json!("yes"))).await.unwrap();
        assert_eq!(finished.stop_reason(), StopReason::EndTurn);
        assert!(executed.load(Ordering::SeqCst));
    }

    #[tokio::test]
    async fn runs_an_approved_tool_only_once_when_the_resume_is_cancelled_while_the_tool_is_in_flight() {
        let model = mock(vec![
            vec![tool_use("sendEmail", "tool-1", json!({}))],
            vec![text("Done")],
            vec![text("Nothing else happened")],
        ]);
        let raised = Arc::new(AtomicBool::new(false));
        let side_effects = Arc::new(Mutex::new(Vec::new()));
        let started = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let (tool_raised, tool_effects, tool_started, tool_release) = (
            Arc::clone(&raised),
            Arc::clone(&side_effects),
            Arc::clone(&started),
            Arc::clone(&release),
        );
        let tool = FunctionTool::new(
            "sendEmail",
            "sends",
            move |_input: JsonValue, ctx: ToolContext| {
                let (raised, effects, started, release) = (
                    Arc::clone(&tool_raised),
                    Arc::clone(&tool_effects),
                    Arc::clone(&tool_started),
                    Arc::clone(&tool_release),
                );
                async move {
                    if !raised.swap(true, Ordering::SeqCst) {
                        ctx.interrupt(InterruptParams {
                            name: "confirm".into(),
                            reason: Some(json!("Send the email?")),
                            ..Default::default()
                        })?;
                    }
                    effects.lock().unwrap().push("email sent");
                    started.notify_one();
                    release.notified().await;
                    Ok::<_, ToolError>("sent")
                }
            },
        );
        let mut agent = Agent::builder().model(model).tools([tool]).build().unwrap();
        let interrupted = agent.invoke("Send the email").await.unwrap();
        let responses = respond(&interrupted, &json!("yes"));

        let handle = agent.cancel_handle();
        let inflight = tokio::spawn(async move {
            let result = agent.invoke(responses).await;
            (agent, result)
        });
        started.notified().await;
        handle.cancel(CancelOptions::default());
        release.notify_one();
        let (mut agent, result) = inflight.await.unwrap();
        assert_eq!(result.unwrap().stop_reason(), StopReason::Cancelled);
        assert!(!has_pending(&agent));
        let tool_use_ids: Vec<&str> = agent
            .messages()
            .iter()
            .flat_map(Message::content)
            .filter_map(|block| match block {
                ContentBlock::ToolUse(tool_use) => Some(tool_use.tool_use_id()),
                _ => None,
            })
            .collect();
        assert_eq!(tool_use_ids, ["tool-1"]);

        agent.invoke("what happened?").await.unwrap();
        assert_eq!(*side_effects.lock().unwrap(), ["email sent"]);
    }
}

mod stream_break_at_interrupt_event {
    use super::*;
    use crate::interrupt::InterruptParams;

    /// A streaming consumer that stops once the interrupt surfaces and returns its id.
    async fn stream_until_interrupt(agent: &mut Agent, prompt: &str) -> String {
        let mut stream = agent.stream(prompt);
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::Interrupt(event) = event.unwrap() {
                return event.interrupt().id().to_owned();
            }
        }
        panic!("stream ended without an InterruptEvent")
    }

    fn model() -> Arc<MockMessageModel> {
        mock(vec![
            vec![tool_use("deleteFiles", "tool-1", json!({}))],
            vec![text("Done")],
        ])
    }

    fn confirming_tool(runs: &Arc<Mutex<Vec<String>>>) -> FunctionTool {
        let runs = Arc::clone(runs);
        FunctionTool::new(
            "deleteFiles",
            "deletes",
            move |_input: JsonValue, ctx: ToolContext| {
                let runs = Arc::clone(&runs);
                async move {
                    let response = ctx.interrupt(InterruptParams {
                        name: "confirm".into(),
                        reason: Some(json!("Delete?")),
                        ..Default::default()
                    })?;
                    runs.lock()
                        .unwrap()
                        .push(response.as_str().unwrap_or_default().to_owned());
                    Ok::<_, ToolError>("deleted")
                }
            },
        )
    }

    #[tokio::test]
    async fn rejects_a_new_prompt_instead_of_replaying_the_stored_tool_cycle() {
        let model = model();
        let runs = Arc::new(Mutex::new(Vec::new()));
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([confirming_tool(&runs)])
            .build()
            .unwrap();
        stream_until_interrupt(&mut agent, "Delete old files").await;

        let error = agent.invoke("anything new").await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "Agent is in an interrupted state. Resume by invoking with interruptResponse content blocks."
        );
        assert!(runs.lock().unwrap().is_empty());
        assert_eq!(model.call_count(), 1);
    }

    #[tokio::test]
    async fn runs_a_tool_approved_call_once_when_resumed() {
        let model = model();
        let runs = Arc::new(Mutex::new(Vec::new()));
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([confirming_tool(&runs)])
            .build()
            .unwrap();
        let id = stream_until_interrupt(&mut agent, "Delete old files").await;
        // The paused turn is not closed with a cancel message.
        assert_eq!(agent.messages().last().unwrap().role(), Role::User);

        let result = agent
            .invoke(vec![InterruptResponse::new(id, json!("yes"))])
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*runs.lock().unwrap(), ["yes"]);
        assert_eq!(model.call_count(), 2);
    }

    #[tokio::test]
    async fn runs_a_hook_approved_call_once_when_resumed() {
        let model = model();
        let log = Arc::new(Mutex::new(Vec::new()));
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([logging_tool("deleteFiles", &log)])
            .build()
            .unwrap();
        agent.add_hook(|event: &mut BeforeToolCallEvent| {
            let approval = event.interrupt(InterruptParams {
                name: "approve_delete".into(),
                reason: Some(json!("Delete?")),
                ..Default::default()
            });
            if approval.is_some_and(|approval| approval != json!("yes")) {
                event.set_cancel("not approved");
            }
        });
        let id = stream_until_interrupt(&mut agent, "Delete old files").await;

        let result = agent
            .invoke(vec![InterruptResponse::new(id, json!("yes"))])
            .await
            .unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*log.lock().unwrap(), ["deleteFiles"]);
        assert_eq!(model.call_count(), 2);
    }
}
