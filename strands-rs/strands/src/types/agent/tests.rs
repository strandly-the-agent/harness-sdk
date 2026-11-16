//! Tests for the agent views and handles. TS has no dedicated spec for them; the
//! `agent.invocation-state` cases are ported with the agent loop.

use std::sync::Arc;

use serde_json::json;

use super::*;
use crate::hooks::{BeforeToolCallEvent, HookRegistry};
use crate::testing::{HookRecorder, MockMessageModel};

fn local_agent() -> LocalAgent {
    LocalAgent::new(Arc::new(MockMessageModel::new()))
}

fn assert_send_sync<T: Send + Sync>() {}

#[test]
fn views_and_handles_are_send_and_sync() {
    assert_send_sync::<LocalAgent>();
    assert_send_sync::<ToolAgent>();
    assert_send_sync::<InvocationState>();
    assert_send_sync::<AgentResult>();
    assert_send_sync::<InvokeArgs>();
    assert_send_sync::<HookRegistry>();
    assert_send_sync::<crate::tools::ToolContext>();
    assert_send_sync::<crate::hooks::HookCleanup>();
    assert_send_sync::<BeforeToolCallEvent>();
    assert_send_sync::<crate::hooks::BeforeInvocationEvent<'static>>();
}

#[test]
fn invocation_state_stores_one_value_per_type_and_clones_share_it() {
    #[derive(Clone, Debug, PartialEq)]
    struct Tenant(&'static str);

    let state = InvocationState::new();
    assert!(state.is_empty());
    let shared = state.clone();
    assert_eq!(state.insert(Tenant("a")), None);
    assert_eq!(state.insert(Tenant("b")), Some(Tenant("a")));
    assert_eq!(shared.get::<Tenant>(), Some(Tenant("b")));
    assert_eq!(shared.get::<u32>(), None);
    assert_eq!(shared.remove::<Tenant>(), Some(Tenant("b")));
    state.insert(1_u8);
    shared.clear();
    assert!(state.is_empty());
}

#[test]
fn a_detached_invocation_state_copies_the_values_and_stops_sharing() {
    let state = InvocationState::new();
    state.insert("alice");
    let copy = state.detached();
    copy.insert(7_u8);
    copy.insert("bob");
    assert_eq!(copy.get::<&str>(), Some("bob"));
    assert_eq!(state.get::<&str>(), Some("alice"));
    assert_eq!(state.get::<u8>(), None);
}

#[test]
fn local_agent_defaults_match_ts() {
    let agent = local_agent();
    assert_eq!(agent.id(), "agent");
    assert_eq!(agent.name(), "Strands Agent");
    assert_eq!(agent.description(), None);
    assert!(agent.messages().is_empty());
    assert!(Arc::ptr_eq(agent.aux_model(), agent.model()));
}

#[test]
fn aux_model_falls_back_to_the_model() {
    let mut agent = local_agent();
    let aux: Arc<dyn Model> = Arc::new(MockMessageModel::new());
    agent.set_aux_model(Some(Arc::clone(&aux)));
    assert!(Arc::ptr_eq(agent.aux_model(), &aux));
    assert!(Arc::ptr_eq(agent.tool_agent().aux_model(), &aux));
    agent.set_aux_model(None);
    assert!(Arc::ptr_eq(agent.aux_model(), agent.model()));
}

#[test]
fn tool_agent_snapshots_history_and_shares_state() {
    let mut agent = local_agent();
    agent.set_messages(vec![Message::new(crate::types::messages::Role::User, Vec::new())]);
    let view = agent.tool_agent();
    agent.messages_mut().clear();
    assert_eq!(view.messages().len(), 1);
    view.app_state().set("k", 1).unwrap();
    assert_eq!(agent.app_state().get::<u32>("k").unwrap(), Some(1));
    view.model_state().set("m", "x").unwrap();
    assert_eq!(agent.model_state().get_value("m"), Some(json!("x")));
}

#[test]
fn cancel_reaches_tool_agent_signal() {
    let mut agent = local_agent();
    agent.begin_cancel(None);
    let view = agent.tool_agent();
    agent.cancel(CancelOptions {
        message: Some("stop".into()),
        ..Default::default()
    });
    assert!(view.cancel_signal().is_cancelled());
    assert_eq!(view.cancel_signal().reason().as_deref(), Some("stop"));
}

#[test]
fn cancel_is_a_no_op_while_idle() {
    let agent = local_agent();
    let view = agent.tool_agent();
    agent.cancel(CancelOptions::default());
    view.cancel(CancelOptions::default());
    assert!(!view.cancel_signal().is_cancelled() && !agent.cancel_signal().is_cancelled());
}

#[test]
fn interrupt_state_returns_the_stored_response_or_the_registered_interrupt() {
    let state = InterruptState::default();
    let raised = state
        .interrupt(
            "tool:t1:confirm".into(),
            InterruptParams {
                reason: Some(json!("why")),
                ..InterruptParams::new("confirm")
            },
            InterruptSource::Tool,
        )
        .unwrap_err();
    assert_eq!(
        serde_json::to_value(&raised).unwrap(),
        json!({"id": "tool:t1:confirm", "name": "confirm", "reason": "why", "source": "tool"})
    );
    state
        .lock()
        .interrupts
        .get_mut("tool:t1:confirm")
        .unwrap()
        .set_response(json!(null));
    let answered = state.interrupt(
        "tool:t1:confirm".into(),
        InterruptParams::new("confirm"),
        InterruptSource::Tool,
    );
    assert_eq!(answered, Ok(json!(null)));
}

#[test]
fn interrupt_params_response_answers_immediately() {
    let state = InterruptState::default();
    let answered = state.interrupt(
        "hook:beforeTools:go".into(),
        InterruptParams {
            response: Some(json!(true)),
            ..InterruptParams::new("go")
        },
        InterruptSource::Hook,
    );
    assert_eq!(answered, Ok(json!(true)));
}

#[test]
fn invoke_args_conversions() {
    assert_eq!(InvokeArgs::from("hi"), InvokeArgs::Text("hi".into()));
    assert_eq!(
        InvokeArgs::from(vec![InterruptResponseContent::new(InterruptResponse::new(
            "id",
            json!(1)
        ))]),
        InvokeArgs::InterruptResponses(vec![InterruptResponse::new("id", json!(1))])
    );
    let message = Message::new(crate::types::messages::Role::User, Vec::new());
    assert_eq!(
        InvokeArgs::from(message.clone()),
        InvokeArgs::Messages(vec![message])
    );
}

#[tokio::test]
async fn hook_recorder_records_events_in_firing_order() {
    let mut agent = local_agent();
    let recorder = HookRecorder::new();
    recorder.register(agent.hooks());
    let hooks = agent.hooks().clone();
    let mut event = crate::hooks::BeforeInvocationEvent::new(&mut agent, InvocationState::new(), Vec::new());
    hooks
        .invoke::<crate::hooks::BeforeInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
    let mut event = crate::hooks::AfterInvocationEvent::new(&mut agent, InvocationState::new());
    hooks
        .invoke::<crate::hooks::AfterInvocationEvent<'static>>(&mut event)
        .await
        .unwrap();
    assert_eq!(
        recorder.events(),
        ["beforeInvocationEvent", "afterInvocationEvent"]
    );
    recorder.clear();
    assert!(recorder.events().is_empty());
}

/// Ported from strands-ts/src/types/__tests__/agent.test.ts (`AgentResult` `toJSON`; the traces and
/// metrics cases land with `AgentMetrics`).
mod agent_result_to_json {
    use super::*;
    use crate::types::messages::{Role, TextBlock};

    fn result() -> AgentResult {
        let message = Message::new(
            Role::Assistant,
            vec![ContentBlock::Text(TextBlock::new("Response"))],
        );
        AgentResult::new(StopReason::EndTurn, message, InvocationState::new())
    }

    #[test]
    fn includes_structured_output_when_present() {
        let json = serde_json::to_value(result().with_structured_output(json!({"field": "value"}))).unwrap();
        assert_eq!(json["structuredOutput"], json!({"field": "value"}));
    }

    #[test]
    fn excludes_structured_output_when_not_present() {
        let json = serde_json::to_value(result()).unwrap();
        assert_eq!(json["type"], "agentResult");
        assert!(json.get("structuredOutput").is_none());
    }
}

/// Ported from strands-ts/src/types/__tests__/agent.test.ts (`toString`, `contextSize`,
/// `projectedContextSize`, the remaining `toJSON` cases; `checkpoint` is out of scope).
mod agent_result {
    use super::*;
    use crate::models::streaming::{ModelMetadataEvent, Usage};
    use crate::telemetry::AgentMetrics;
    use crate::telemetry::meter::Meter;
    use crate::types::citations::{CitationGeneratedContent, CitationsBlock};
    use crate::types::media::ImageBlock;
    use crate::types::messages::{ReasoningBlock, Role, TextBlock, ToolUseBlock};

    fn with_content(content: Vec<ContentBlock>) -> AgentResult {
        AgentResult::new(
            StopReason::EndTurn,
            Message::new(Role::Assistant, content),
            InvocationState::new(),
        )
    }

    fn text(text: &str) -> ContentBlock {
        TextBlock::new(text).into()
    }

    fn metrics_with_usage(usage: Usage) -> AgentMetrics {
        let mut meter = Meter::default();
        meter.start_new_invocation();
        meter.start_cycle();
        meter.update_cycle(Some(&ModelMetadataEvent::new().with_usage(usage)));
        meter.metrics().clone()
    }

    #[test]
    fn returns_empty_string() {
        assert_eq!(with_content(vec![]).to_string(), "");
    }

    #[test]
    fn to_string_joins_text_blocks_with_newlines() {
        assert_eq!(
            with_content(vec![text("Hello, world!")]).to_string(),
            "Hello, world!"
        );
        assert_eq!(
            with_content(vec![text("First"), text("Second"), text("Third")]).to_string(),
            "First\nSecond\nThird"
        );
    }

    #[test]
    fn to_string_prefixes_and_indents_reasoning_and_skips_empty_reasoning() {
        let reasoning: ContentBlock = ReasoningBlock::new().with_text("Line 1\nLine 2").into();
        assert_eq!(
            with_content(vec![reasoning]).to_string(),
            "💭 Reasoning:\n   Line 1\n   Line 2"
        );
        let signed: ContentBlock = ReasoningBlock::new().with_signature("sig").into();
        assert_eq!(with_content(vec![signed]).to_string(), "");
        let mixed = vec![
            text("Answer"),
            ReasoningBlock::new().with_text("why").into(),
            text("More"),
        ];
        assert_eq!(
            with_content(mixed).to_string(),
            "Answer\n💭 Reasoning:\n   why\nMore"
        );
    }

    #[test]
    fn to_string_skips_non_text_blocks() {
        let tool_use: ContentBlock = ToolUseBlock::new("t", "id", json!({})).into();
        let image: ContentBlock = ImageBlock::new(crate::mime::ImageFormat::Png, vec![1, 2, 3]).into();
        assert_eq!(with_content(vec![tool_use.clone(), image]).to_string(), "");
        assert_eq!(
            with_content(vec![tool_use, text("Visible")]).to_string(),
            "Visible"
        );
    }

    #[test]
    fn concatenates_generated_content_text_from_citations() {
        let citations = CitationsBlock::new(
            vec![],
            vec![
                CitationGeneratedContent::new("Cited one."),
                CitationGeneratedContent::new("Cited two."),
            ],
        );
        assert_eq!(
            with_content(vec![citations.into(), text("Tail")]).to_string(),
            "Cited one.\nCited two.\nTail"
        );
    }

    #[test]
    fn to_string_prefers_interrupts_then_structured_output() {
        let interrupt = Interrupt::new("i-1", "confirm").with_reason(json!("ok?"));
        let result = with_content(vec![text("ignored")]).with_interrupts(vec![interrupt.clone()]);
        assert_eq!(
            result.to_string(),
            to_js_string(std::slice::from_ref(&interrupt)).unwrap()
        );
        assert_eq!(
            with_content(vec![text("Hello")])
                .with_interrupts(vec![])
                .to_string(),
            "Hello"
        );
        let structured =
            with_content(vec![text("ignored")]).with_structured_output(json!({"a": 1, "b": [2]}));
        assert_eq!(structured.to_string(), r#"{"a":1,"b":[2]}"#);
        let both = structured.with_interrupts(vec![interrupt.clone()]);
        assert_eq!(both.to_string(), to_js_string(&[interrupt]).unwrap());
        assert_eq!(
            format!("Result: {}", with_content(vec![text("Hi")])),
            "Result: Hi"
        );
    }

    #[test]
    fn context_sizes_come_from_the_metrics() {
        let usage = Usage::new(100, 20, 120);
        let result = with_content(vec![]).with_telemetry(metrics_with_usage(usage), vec![]);
        assert_eq!(result.context_size(), Some(100));
        assert_eq!(result.projected_context_size(), Some(120));
        let empty = with_content(vec![]);
        assert_eq!(
            (empty.context_size(), empty.projected_context_size()),
            (None, None)
        );
    }

    #[test]
    fn to_json_excludes_traces_metrics_and_interrupts_but_keeps_them_readable() {
        let result = with_content(vec![text("Hello")])
            .with_telemetry(metrics_with_usage(Usage::new(1, 2, 3)), vec![])
            .with_interrupts(vec![Interrupt::new("i-1", "confirm")]);
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({"type": "agentResult", "stopReason": "endTurn", "lastMessage": serde_json::to_value(result.last_message()).unwrap()})
        );
        assert_eq!(result.metrics().cycle_count(), 1);
        assert_eq!(result.interrupts().len(), 1);
    }

    #[test]
    fn structured_output_deserializes_into_the_requested_type() {
        #[derive(serde::Deserialize, Debug, PartialEq)]
        struct Weather {
            city: String,
        }
        let result = with_content(vec![]).with_structured_output(json!({"city": "Lisbon"}));
        assert_eq!(
            result.structured_output::<Weather>().unwrap(),
            Weather {
                city: "Lisbon".into()
            }
        );
        assert_eq!(
            result.structured_output::<u32>().unwrap_err().name(),
            "StructuredOutputError"
        );
        assert_eq!(
            with_content(vec![])
                .structured_output::<Weather>()
                .unwrap_err()
                .name(),
            "StructuredOutputError"
        );
    }
}

mod call_tool {
    //! `ToolAgent::call_tool`; TS has no public counterpart (its programmatic tool caller reaches
    //! the agent's private hook registry).

    use std::sync::Mutex;

    use super::*;
    use crate::hooks::AfterToolCallEvent;
    use crate::tools::{FunctionTool, ToolError};
    use crate::types::tools::ToolResultStatus;

    fn agent_with_echo(ran: &Arc<Mutex<Vec<JsonValue>>>) -> LocalAgent {
        let ran = Arc::clone(ran);
        let echo = FunctionTool::new("echo", "test tool", move |input: JsonValue, _context| {
            ran.lock().unwrap().push(input.clone());
            async move { Ok::<_, ToolError>(format!("echo:{}", input["value"])) }
        });
        let mut agent = local_agent();
        agent.tool_registry_mut().add(Arc::new(echo)).unwrap();
        agent
    }

    fn result_of(outcome: ToolCallOutcome) -> JsonValue {
        match outcome {
            ToolCallOutcome::Result(result) => json!({
                "status": serde_json::to_value(result.status()).unwrap(),
                "content": serde_json::to_value(result.content()).unwrap(),
            }),
            other => panic!("expected a result, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn runs_the_tool_between_the_before_and_after_hooks() {
        let ran = Arc::default();
        let agent = agent_with_echo(&ran);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = Arc::clone(&seen);
        let _before = agent.hooks().add_hook(move |event: &mut BeforeToolCallEvent| {
            log.lock()
                .unwrap()
                .push(format!("before:{}", event.tool_use().name()));
        });
        let log = Arc::clone(&seen);
        let _after = agent.hooks().add_hook(move |event: &mut AfterToolCallEvent| {
            log.lock()
                .unwrap()
                .push(format!("after:{}", event.tool_use().name()));
        });

        let outcome = agent
            .tool_agent()
            .call_tool("echo", json!({"value": 1}), InvocationState::new())
            .await
            .unwrap();

        assert_eq!(
            result_of(outcome),
            json!({"status": "success", "content": [{"text": "echo:1"}]})
        );
        assert_eq!(*seen.lock().unwrap(), ["before:echo", "after:echo"]);
        assert_eq!(*ran.lock().unwrap(), [json!({"value": 1})]);
    }

    #[tokio::test]
    async fn a_hook_cancel_is_an_error_result_and_the_tool_never_runs() {
        let ran = Arc::default();
        let agent = agent_with_echo(&ran);
        let _before = agent
            .hooks()
            .add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel("not allowed"));

        let outcome = agent
            .tool_agent()
            .call_tool("echo", json!({}), InvocationState::new())
            .await
            .unwrap();

        let ToolCallOutcome::HookCancelled(result) = outcome else {
            panic!("expected a hook cancel, got {outcome:?}");
        };
        assert_eq!(
            result_of(ToolCallOutcome::Result(result)),
            json!({"status": "error", "content": [{"text": "not allowed"}]})
        );
        assert!(ran.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_hook_interrupt_is_returned_and_never_recorded_in_the_agent_interrupt_state() {
        let ran = Arc::default();
        let agent = agent_with_echo(&ran);
        let _before = agent.hooks().add_hook(|event: &mut BeforeToolCallEvent| {
            let _response = event.interrupt(InterruptParams {
                reason: Some(json!("why")),
                ..InterruptParams::new("confirm")
            });
        });
        let view = agent.tool_agent();

        let outcome = view
            .call_tool("echo", json!({}), InvocationState::new())
            .await
            .unwrap();

        let ToolCallOutcome::Interrupt(interrupt) = outcome else {
            panic!("expected an interrupt, got {outcome:?}");
        };
        assert_eq!(interrupt.name(), "confirm");
        assert_eq!(interrupt.reason(), Some(&json!("why")));
        assert!(view.interrupt_state().lock().interrupts.is_empty());
        assert!(ran.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn a_cancelled_agent_makes_no_call() {
        let ran = Arc::default();
        let mut agent = agent_with_echo(&ran);
        agent.begin_cancel(None);
        agent.cancel(CancelOptions::default());

        let outcome = agent
            .tool_agent()
            .call_tool("echo", json!({}), InvocationState::new())
            .await
            .unwrap();

        assert!(matches!(outcome, ToolCallOutcome::Cancelled), "{outcome:?}");
        assert!(ran.lock().unwrap().is_empty());
    }

    #[tokio::test]
    async fn an_unknown_tool_is_an_error_result() {
        let agent = agent_with_echo(&Arc::default());

        let outcome = agent
            .tool_agent()
            .call_tool("missing", json!({}), InvocationState::new())
            .await
            .unwrap();

        let ToolCallOutcome::Result(result) = outcome else {
            panic!("expected a result, got {outcome:?}");
        };
        assert_eq!(*result.status(), ToolResultStatus::Error);
    }

    /// Review finding correctness-a2: the middleware's own error, not "ended without a result".
    #[tokio::test]
    async fn a_failing_execute_tool_middleware_surfaces_its_error() {
        use crate::middleware::ExecuteToolStage;

        let ran = Arc::default();
        let agent = agent_with_echo(&ran);
        let _cleanup = agent.add_middleware(ExecuteToolStage::wrap(|_context, _next| {
            Box::pin(futures::stream::iter([Err(crate::Error::other(
                "middleware failed",
            )
            .into())]))
        }));

        let error = agent
            .tool_agent()
            .call_tool("echo", json!({}), InvocationState::new())
            .await
            .unwrap_err();

        assert!(error.to_string().contains("middleware failed"), "{error}");
        assert!(ran.lock().unwrap().is_empty());
    }
}
