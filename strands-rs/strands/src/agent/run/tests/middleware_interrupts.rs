//! Middleware interrupts, ported from
//! strands-ts/src/middleware/__tests__/middleware-interrupts.test.ts.

use futures::stream;

use super::*;
use crate::agent::SnapshotField;
use crate::agent::snapshot::TakeSnapshotOptions;
use crate::interrupt::{InterruptParams, InterruptResponse, InterruptSource};
use crate::middleware::{
    AgentStreamResult, AgentStreamStage, ExecuteToolResult, ExecuteToolStage, MiddlewareItem,
    MiddlewareStage, MiddlewareStream,
};
use crate::test_support::{mock_tool, text_result};

fn counting_tool(name: &str, calls: &Arc<Mutex<usize>>) -> crate::test_support::MockTool {
    let calls = Arc::clone(calls);
    mock_tool(name, move |ctx| {
        *calls.lock().unwrap() += 1;
        text_result(ctx.tool_use().tool_use_id(), "executed")
    })
}

fn agent_with_tool(model: &Arc<MockMessageModel>, tool: crate::test_support::MockTool) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .tools([tool])
        .printer(false)
        .build()
        .unwrap()
}

fn plain_agent(model: &Arc<MockMessageModel>) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .printer(false)
        .build()
        .unwrap()
}

fn tool_then(name: &str, id: &str, then: &str) -> Arc<MockMessageModel> {
    mock(vec![vec![tool_use(name, id, json!({}))], vec![text(then)]])
}

fn params(name: &str, reason: Option<&str>) -> InterruptParams {
    InterruptParams {
        name: name.to_owned(),
        reason: reason.map(|reason| json!(reason)),
        response: None,
    }
}

/// An `ExecuteToolStage` gate: interrupts, and on resume runs the tool only for `allow`.
fn tool_gate(name: &'static str, allow: &'static str) -> crate::middleware::Middleware<ExecuteToolStage> {
    ExecuteToolStage::wrap(move |context, next| -> MiddlewareStream<'_, ExecuteToolStage> {
        match context.interrupt(params(name, Some("Confirm?"))) {
            Err(interrupt) => Box::pin(stream::iter([Err(interrupt.into())])),
            Ok(answer) if answer.response() == &json!(allow) => next.run(context),
            Ok(_) => {
                let result = ToolResultBlock::new(
                    context.tool_use.tool_use_id(),
                    ToolResultStatus::Error,
                    vec!["Denied by user".into()],
                );
                Box::pin(stream::iter([Ok(MiddlewareItem::Result(
                    ExecuteToolResult::new(result),
                ))]))
            }
        }
    })
}

/// An `AgentStreamStage` handler that only interrupts.
fn stream_gate(
    name: &'static str,
    reason: Option<&'static str>,
) -> crate::middleware::Middleware<AgentStreamStage> {
    AgentStreamStage::wrap(move |context, _| {
        let error = match context.interrupt(params(name, reason)) {
            Err(interrupt) => interrupt.into(),
            Ok(_) => Error::other("unexpected response").into(),
        };
        Box::pin(stream::iter([Err(error)]))
    })
}

fn respond(result: &AgentResult, response: JsonValue) -> Vec<InterruptResponse> {
    vec![InterruptResponse::new(result.interrupts()[0].id(), response)]
}

mod execute_tool_stage {
    use super::*;

    #[tokio::test]
    async fn middleware_can_raise_an_interrupt_agent_stops_with_stopreason_interrupt() {
        let model = tool_then("dangerousTool", "tool-1", "Should not reach");
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("dangerousTool", &calls));
        agent.add_middleware(ExecuteToolStage::wrap(|context, next| {
            if let Err(interrupt) = context.interrupt(params("approve_tool", Some("Confirm execution?"))) {
                return Box::pin(stream::iter([Err(interrupt.into())]));
            }
            next.run(context)
        }));
        let result = agent.invoke("Do the dangerous thing").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        let interrupt = &result.interrupts()[0];
        assert_eq!(result.interrupts().len(), 1);
        assert_eq!(interrupt.name(), "approve_tool");
        assert_eq!(interrupt.reason(), Some(&json!("Confirm execution?")));
        assert_eq!(*interrupt.source(), InterruptSource::Middleware);
        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn middleware_gets_response_on_resume_and_continues_execution() {
        let model = tool_then("dangerousTool", "tool-1", "Done");
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("dangerousTool", &calls));
        agent.add_middleware(tool_gate("approve_tool", "yes"));
        let interrupted = agent.invoke("Do it").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        assert_eq!(*calls.lock().unwrap(), 0);
        let result = agent.invoke(respond(&interrupted, json!("yes"))).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn uses_interrupt_state_restored_after_executor_construction() {
        let model = tool_then("dangerousTool", "tool-1", "Done");
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("dangerousTool", &calls));
        agent.add_middleware(tool_gate("approve_tool", "yes"));
        let options = TakeSnapshotOptions {
            include: vec![SnapshotField::Interrupts],
            ..TakeSnapshotOptions::default()
        };
        let snapshot = agent.take_snapshot(&options).unwrap();
        agent.load_snapshot(&snapshot).unwrap();
        let interrupted = agent.invoke("Do it").await.unwrap();
        let result = agent.invoke(respond(&interrupted, json!("yes"))).await.unwrap();
        assert_eq!(
            (
                interrupted.stop_reason(),
                result.stop_reason(),
                *calls.lock().unwrap()
            ),
            (StopReason::Interrupt, StopReason::EndTurn, 1)
        );
    }

    #[tokio::test]
    async fn interrupt_id_includes_tooluseid_for_disambiguation() {
        let model = tool_then("myTool", "unique-tool-id", "Done");
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("myTool", &calls));
        agent.add_middleware(tool_gate("check", "yes"));
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(
            result.interrupts()[0].id(),
            "middleware:executeTool:unique-tool-id:check"
        );
    }

    #[tokio::test]
    async fn preemptive_response_skips_the_interrupt_no_halt() {
        let model = tool_then("myTool", "tool-1", "Done");
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("myTool", &calls));
        let seen: Arc<Mutex<Option<JsonValue>>> = Arc::default();
        let slot = Arc::clone(&seen);
        agent.add_middleware(ExecuteToolStage::wrap(move |context, next| {
            let params = InterruptParams {
                name: "check".to_owned(),
                reason: None,
                response: Some(json!("pre-approved")),
            };
            *slot.lock().unwrap() = context
                .interrupt(params)
                .ok()
                .map(|answer| answer.response().clone());
            next.run(context)
        }));
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(seen.lock().unwrap().take(), Some(json!("pre-approved")));
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn yields_interruptevent_on_the_stream_for_executetoolstage_interrupts() {
        let model = tool_then("myTool", "tool-1", "Done");
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("myTool", &calls));
        agent.add_middleware(tool_gate("tool_gate", "yes"));
        let events = collect(agent.stream("Test")).await;
        let interrupts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentStreamEvent::Interrupt(data) => Some(data.interrupt().name().to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(interrupts, ["tool_gate"]);
    }

    #[tokio::test]
    async fn context_spread_preserves_interrupt_function() {
        let model = mock(vec![
            vec![tool_use("myTool", "tool-1", json!({"x": 1}))],
            vec![text("Done")],
        ]);
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("myTool", &calls));
        agent.add_middleware(ExecuteToolStage::wrap(|context, next| {
            let mut modified = context.clone();
            modified.tool_use = crate::types::tools::ToolUse::new(
                context.tool_use.name(),
                context.tool_use.tool_use_id(),
                json!({"x": 2}),
            );
            if let Err(interrupt) = modified.interrupt(params("after_spread", None)) {
                return Box::pin(stream::iter([Err(interrupt.into())]));
            }
            next.run(modified)
        }));
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        assert_eq!(result.interrupts()[0].name(), "after_spread");
    }
}

mod agent_stream_stage {
    use super::*;

    #[tokio::test]
    async fn middleware_can_raise_an_interrupt_agent_stops_with_stopreason_interrupt() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = plain_agent(&model);
        agent.add_middleware(stream_gate("confirm_stream", Some("Are you sure?")));
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        assert_eq!(result.interrupts().len(), 1);
        assert_eq!(result.interrupts()[0].name(), "confirm_stream");
        assert_eq!(result.interrupts()[0].reason(), Some(&json!("Are you sure?")));
        // The user message was never appended: the pass didn't run.
        assert!(agent.messages().is_empty());
        assert_eq!(result.last_message().content(), [text("Interrupted")]);
    }

    #[tokio::test]
    async fn interrupt_lastmessage_uses_last_existing_message_when_messages_exist() {
        let model = mock(vec![vec![text("First response")], vec![text("Should not reach")]]);
        let mut agent = plain_agent(&model);
        agent.invoke("Hello").await.unwrap();
        agent.add_middleware(stream_gate("gate", None));
        let result = agent.invoke("Second call").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::Interrupt);
        assert_eq!(result.last_message().content()[0], text("First response"));
    }

    #[tokio::test]
    async fn middleware_gets_response_on_resume_and_continues() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = plain_agent(&model);
        agent.add_middleware(AgentStreamStage::wrap(|context, next| {
            match context.interrupt(params("gate", Some("Proceed?"))) {
                Err(interrupt) => Box::pin(stream::iter([Err(interrupt.into())])),
                Ok(answer) if answer.response() == &json!("go") => next.run(context),
                Ok(_) => {
                    let result = AgentResult::new(
                        StopReason::EndTurn,
                        Message::new(Role::Assistant, Vec::new()),
                        InvocationState::new(),
                    );
                    Box::pin(stream::iter([Ok(MiddlewareItem::Result(
                        AgentStreamResult::new(result),
                    ))]))
                }
            }
        }));
        let interrupted = agent.invoke("Test").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        let result = agent.invoke(respond(&interrupted, json!("go"))).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn clears_interrupt_state_after_resuming_to_a_non_tool_completion_so_the_agent_is_reusable() {
        let model = mock(vec![vec![text("Resumed")], vec![text("Fresh")]]);
        let mut agent = plain_agent(&model);
        let interrupted_once = Arc::new(Mutex::new(false));
        agent.add_middleware(AgentStreamStage::wrap(move |context, next| {
            let mut interrupted = interrupted_once.lock().unwrap();
            if !*interrupted {
                *interrupted = true;
                if let Err(interrupt) = context.interrupt(params("gate", None)) {
                    return Box::pin(stream::iter([Err(interrupt.into())]));
                }
            }
            next.run(context)
        }));
        let interrupted = agent.invoke("Test").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        let resumed = agent.invoke(respond(&interrupted, json!("go"))).await.unwrap();
        assert_eq!(resumed.stop_reason(), StopReason::EndTurn);
        let fresh = agent.invoke("Fresh call").await.unwrap();
        assert_eq!(fresh.stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn re_reads_the_resumed_response_after_next_even_when_a_tool_ran_in_the_pass() {
        let model = tool_then("calc", "tool-1", "done");
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("calc", &calls));
        let reads: Arc<Mutex<Vec<(&'static str, JsonValue)>>> = Arc::default();
        let log = Arc::clone(&reads);
        agent.add_middleware(AgentStreamStage::wrap(move |context, next| {
            let pre = match context.interrupt(params("gate", None)) {
                Ok(answer) => answer.response().clone(),
                Err(interrupt) => return Box::pin(stream::iter([Err(interrupt.into())])),
            };
            log.lock().unwrap().push(("pre", pre));
            let log = Arc::clone(&log);
            let mut inner = next.run(context.clone());
            Box::pin(async_stream::stream! {
                while let Some(item) = inner.next().await {
                    yield item;
                }
                if let Ok(answer) = context.interrupt(params("gate", None)) {
                    log.lock().unwrap().push(("post", answer.response().clone()));
                }
            })
        }));
        let interrupted = agent.invoke("go").await.unwrap();
        assert_eq!(interrupted.stop_reason(), StopReason::Interrupt);
        let result = agent.invoke(respond(&interrupted, json!("go"))).await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(
            *reads.lock().unwrap(),
            [("pre", json!("go")), ("post", json!("go"))]
        );
    }

    #[tokio::test]
    async fn interrupt_id_uses_agentstream_namespace() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = plain_agent(&model);
        agent.add_middleware(stream_gate("my_gate", None));
        let result = agent.invoke("Test").await.unwrap();
        assert_eq!(result.interrupts()[0].id(), "middleware:agentStream:my_gate");
    }

    #[tokio::test]
    async fn yields_interruptevent_on_the_stream_for_agentstreamstage_interrupts() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = plain_agent(&model);
        agent.add_middleware(stream_gate("stream_gate", Some("confirm?")));
        let events = collect(agent.stream("Test")).await;
        let interrupts: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                AgentStreamEvent::Interrupt(data) => Some(data.interrupt().name().to_owned()),
                _ => None,
            })
            .collect();
        assert_eq!(interrupts, ["stream_gate"]);
    }
}

#[tokio::test]
async fn afterinvocationevent_hook_can_resume_a_tool_level_interrupt_in_middleware() {
    let model = tool_then("myTool", "tool-1", "Done after resume");
    let calls = Arc::new(Mutex::new(0));
    let mut agent = agent_with_tool(&model, counting_tool("myTool", &calls));
    agent.add_middleware(tool_gate("gate", "approved"));
    agent.add_hook(|event: &mut AfterInvocationEvent<'_>| {
        let unanswered = event
            .agent()
            .interrupt_state()
            .lock()
            .unanswered()
            .next()
            .map(|interrupt| interrupt.id().to_owned());
        if let Some(id) = unanswered {
            event.set_resume(vec![InterruptResponse::new(id, json!("approved"))]);
        }
    });
    let result = agent.invoke("Do it").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(*calls.lock().unwrap(), 1);
}
