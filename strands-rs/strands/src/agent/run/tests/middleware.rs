//! Middleware in the agent loop, ported from
//! strands-ts/src/middleware/__tests__/agent-middleware.test.ts.

use futures::stream;

use super::*;
use crate::hooks::{AfterToolCallEvent, BeforeToolCallEvent};
use crate::middleware::{
    AgentStreamResult, AgentStreamStage, ExecuteToolContext, ExecuteToolResult, ExecuteToolStage,
    InvokeModelContext, InvokeModelResult, InvokeModelStage, MiddlewareError, MiddlewareItem,
    MiddlewareStage, MiddlewareStream,
};
use crate::models::model::{Model, StreamAggregatedResult};
use crate::plugins::Plugin;
use crate::test_support::{mock_tool, text_result};
use crate::types::agent::{ContentBlockEventData, InvokeArgs, LocalAgent};
use crate::types::tools::ToolUse;

type Log = Arc<Mutex<Vec<String>>>;

fn push(log: &Log, entry: &str) {
    log.lock().unwrap().push(entry.to_owned());
}

fn entries(log: &Log) -> Vec<String> {
    log.lock().unwrap().clone()
}

fn agent_with(model: &Arc<MockMessageModel>) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .printer(false)
        .build()
        .unwrap()
}

fn agent_with_tool(model: &Arc<MockMessageModel>, tool: crate::test_support::MockTool) -> Agent {
    Agent::builder()
        .model(Arc::clone(model))
        .tools([tool])
        .printer(false)
        .build()
        .unwrap()
}

fn ok_tool(name: &str, text: &'static str) -> crate::test_support::MockTool {
    mock_tool(name, move |ctx| text_result(ctx.tool_use().tool_use_id(), text))
}

fn tool_then_done(name: &str, input: JsonValue) -> Arc<MockMessageModel> {
    mock(vec![vec![tool_use(name, "tool-1", input)], vec![text("Done")]])
}

fn assistant(content: &str) -> Message {
    Message::new(Role::Assistant, vec![text(content)])
}

/// A Wrap handler for any stage that logs `before` and `after` around the rest of the chain.
fn around<S: MiddlewareStage>(
    log: &Log,
    before: &'static str,
    after: &'static str,
) -> crate::middleware::Middleware<S>
where
    S::Event: 'static,
{
    let log = Arc::clone(log);
    S::wrap(move |context, next| {
        push(&log, before);
        let log = Arc::clone(&log);
        let mut inner = next.run(context);
        Box::pin(async_stream::stream! {
            while let Some(item) = inner.next().await {
                yield item;
            }
            push(&log, after);
        })
    })
}

fn model_result(message: Message, stop_reason: StopReason) -> MiddlewareStream<'static, InvokeModelStage> {
    let result = InvokeModelResult::new(StreamAggregatedResult::new(message, stop_reason));
    Box::pin(stream::iter([Ok(MiddlewareItem::Result(result))]))
}

fn tool_result(result: ToolResultBlock) -> MiddlewareStream<'static, ExecuteToolStage> {
    Box::pin(stream::iter([Ok(MiddlewareItem::Result(
        ExecuteToolResult::new(result),
    ))]))
}

fn is_hook_record(event: &AgentStreamEvent) -> bool {
    matches!(
        event,
        AgentStreamEvent::BeforeInvocation(_)
            | AgentStreamEvent::AfterInvocation(_)
            | AgentStreamEvent::AgentResult(_)
    )
}

fn last_result(events: &[AgentStreamEvent]) -> &AgentResult {
    match events.last() {
        Some(AgentStreamEvent::AgentResult(result)) => result,
        other => panic!("expected an agent result, got {other:?}"),
    }
}

fn json_of(event: &AgentStreamEvent) -> JsonValue {
    serde_json::to_value(event).unwrap()
}

fn synthetic(text_: &str) -> AgentStreamEvent {
    AgentStreamEvent::ContentBlock(ContentBlockEventData::new(text(text_)))
}

mod invoke_model_stage {
    use super::*;

    #[tokio::test]
    async fn middleware_handler_is_invoked_during_model_call() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let calls = Arc::clone(&log);
        agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
            push(&calls, "called");
            next.run(context)
        }));
        agent.invoke("Test prompt").await.unwrap();
        assert_eq!(entries(&log), ["called"]);
    }

    #[tokio::test]
    async fn middleware_receives_invokemodelcontext_with_correct_fields() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([ok_tool("testTool", "ok")])
            .system_prompt("Be helpful")
            .printer(false)
            .build()
            .unwrap();
        let seen: Arc<Mutex<Option<InvokeModelContext>>> = Arc::default();
        let slot = Arc::clone(&seen);
        agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
            *slot.lock().unwrap() = Some(context.clone());
            next.run(context)
        }));
        agent.invoke("Test prompt").await.unwrap();
        let context = seen.lock().unwrap().take().unwrap();
        assert_eq!(context.agent().id(), agent.id());
        assert!(std::ptr::addr_eq(
            Arc::as_ptr(&context.model),
            Arc::as_ptr(&model)
        ));
        assert_eq!(context.system_prompt, Some(SystemPrompt::from("Be helpful")));
        assert_eq!(context.messages.len(), 1);
        assert!(context.tool_specs.iter().any(|spec| spec.name() == "testTool"));
    }

    #[tokio::test]
    async fn middleware_can_select_the_model_for_one_call() {
        let default_model = mock(vec![vec![text("Default response")]]);
        let selected = mock(vec![vec![text("Selected response")]]);
        let mut agent = agent_with(&default_model);
        let chosen: Arc<dyn Model> = selected.clone();
        agent.add_middleware(InvokeModelStage::input(move |mut context| {
            let chosen = Arc::clone(&chosen);
            async move {
                context.model = chosen;
                Ok(context)
            }
        }));
        let result = agent.invoke("Test prompt").await.unwrap();
        assert_eq!(result.last_message().content(), [text("Selected response")]);
        assert!(default_model.requests().is_empty());
    }

    #[tokio::test]
    async fn middleware_result_is_used_as_the_model_call_result() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(InvokeModelStage::wrap(|context, next| next.run(context)));
        let result = agent.invoke("Test prompt").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().content(), [text("Hello")]);
    }

    #[tokio::test]
    async fn returns_synthetic_result_without_calling_the_model() {
        let model = mock(vec![vec![text("Real response")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(InvokeModelStage::wrap(|_, _| {
            model_result(assistant("Cached response"), StopReason::EndTurn)
        }));
        let result = agent.invoke("Test prompt").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().content(), [text("Cached response")]);
    }

    #[tokio::test]
    async fn model_is_not_called_when_middleware_short_circuits() {
        let model = mock(vec![vec![text("Real response")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(InvokeModelStage::wrap(|_, _| {
            model_result(assistant("Cached"), StopReason::EndTurn)
        }));
        agent.invoke("Test prompt").await.unwrap();
        assert!(model.requests().is_empty());
    }

    #[tokio::test]
    async fn modified_messages_are_passed_to_the_model() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
            context
                .messages
                .push(Message::new(Role::User, vec![text("Injected message")]));
            next.run(context)
        }));
        agent.invoke("Test prompt").await.unwrap();
        assert!(model.requests()[0].messages.len() > 1);
    }

    #[tokio::test]
    async fn modified_toolspecs_are_passed_to_the_model() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "ok"));
        agent.add_middleware(InvokeModelStage::wrap(|mut context, next| {
            context.tool_specs.clear();
            next.run(context)
        }));
        agent.invoke("Test prompt").await.unwrap();
        assert!(model.requests()[0].tool_specs.is_empty());
    }

    #[tokio::test]
    async fn beforemodelcallevent_fires_before_middleware_executes() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let hook_log = Arc::clone(&log);
        agent.add_hook(move |_: &mut BeforeModelCallEvent<'_>| push(&hook_log, "beforeModelCall"));
        let calls = Arc::clone(&log);
        agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
            push(&calls, "middleware");
            next.run(context)
        }));
        agent.invoke("Test prompt").await.unwrap();
        assert_eq!(entries(&log), ["beforeModelCall", "middleware"]);
    }

    #[tokio::test]
    async fn aftermodelcallevent_fires_after_middleware_completes() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let hook_log = Arc::clone(&log);
        agent.add_hook(move |_: &mut AfterModelCallEvent<'_>| push(&hook_log, "afterModelCall"));
        agent.add_middleware(around::<InvokeModelStage>(
            &log,
            "middleware-start",
            "middleware-end",
        ));
        agent.invoke("Test prompt").await.unwrap();
        assert_eq!(
            entries(&log),
            ["middleware-start", "middleware-end", "afterModelCall"]
        );
    }

    #[tokio::test]
    async fn before_after_hooks_fire_even_when_middleware_short_circuits() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let before = Arc::clone(&log);
        agent.add_hook(move |_: &mut BeforeModelCallEvent<'_>| push(&before, "before"));
        let after = Arc::clone(&log);
        agent.add_hook(move |_: &mut AfterModelCallEvent<'_>| push(&after, "after"));
        agent.add_middleware(InvokeModelStage::wrap(|_, _| {
            model_result(assistant("Cached"), StopReason::EndTurn)
        }));
        agent.invoke("Test prompt").await.unwrap();
        assert_eq!(entries(&log), ["before", "after"]);
    }

    #[tokio::test]
    async fn returns_a_function_that_removes_the_middleware() {
        let model = mock(vec![vec![text("First")], vec![text("Second")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let calls = Arc::clone(&log);
        let cleanup = agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
            push(&calls, "called");
            next.run(context)
        }));
        agent.invoke("First call").await.unwrap();
        assert_eq!(entries(&log), ["called"]);
        cleanup.remove();
        agent.invoke("Second call").await.unwrap();
        assert_eq!(entries(&log), ["called"]);
    }

    #[tokio::test]
    async fn only_removes_the_specific_handler_not_others_on_the_same_stage() {
        let model = mock(vec![vec![text("Result")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        for name in ["keeper", "removed"] {
            let calls = Arc::clone(&log);
            let cleanup = agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
                push(&calls, name);
                next.run(context)
            }));
            if name == "removed" {
                cleanup.remove();
            }
        }
        agent.invoke("Test").await.unwrap();
        assert_eq!(entries(&log), ["keeper"]);
    }

    #[tokio::test]
    async fn agent_works_correctly_without_any_middleware() {
        let model = mock(vec![vec![text("Hello")]]);
        let result = agent_with(&model).invoke("Test prompt").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().content(), [text("Hello")]);
    }

    #[tokio::test]
    async fn agent_with_tools_works_correctly_without_middleware() {
        let model = tool_then_done("testTool", json!({}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "Tool executed"));
        let result = agent.invoke("Use the tool").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().content(), [text("Done")]);
    }

    #[tokio::test]
    async fn stream_works_correctly_without_middleware() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let events = collect(agent.stream("Test prompt")).await;
        let result = last_result(&events);
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().content(), [text("Hello")]);
    }
}

mod agent_stream_stage {
    use super::*;

    fn short_circuit_result() -> MiddlewareStream<'static, AgentStreamStage> {
        let result = AgentResult::new(
            StopReason::EndTurn,
            Message::new(Role::Assistant, Vec::new()),
            InvocationState::new(),
        );
        Box::pin(stream::iter([Ok(MiddlewareItem::Result(
            AgentStreamResult::new(result),
        ))]))
    }

    #[tokio::test]
    async fn middleware_executes_around_the_full_agent_stream() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        agent.add_middleware(around::<AgentStreamStage>(
            &log,
            "middleware-before",
            "middleware-after",
        ));
        let events = collect(agent.stream("Test prompt")).await;
        assert_eq!(entries(&log), ["middleware-before", "middleware-after"]);
        assert_eq!(last_result(&events).stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn middleware_receives_agentstreamcontext_with_agent_args_and_options() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let seen: Arc<Mutex<Option<(String, InvokeArgs)>>> = Arc::default();
        let slot = Arc::clone(&seen);
        agent.add_middleware(AgentStreamStage::wrap(move |context, next| {
            *slot.lock().unwrap() = Some((context.agent().id().to_owned(), context.args.clone()));
            next.run(context)
        }));
        collect(agent.stream("Test prompt")).await;
        let (id, args) = seen.lock().unwrap().take().unwrap();
        assert_eq!(id, agent.id());
        assert_eq!(args, InvokeArgs::from("Test prompt"));
    }

    #[tokio::test]
    async fn middleware_receives_options_when_provided() {
        #[derive(Clone)]
        struct Key(&'static str);
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let seen = Log::default();
        let slot = Arc::clone(&seen);
        agent.add_middleware(AgentStreamStage::wrap(move |context, next| {
            if let Some(Key(value)) = context.options.invocation_state.get::<Key>() {
                push(&slot, value);
            }
            next.run(context)
        }));
        let state = InvocationState::new();
        state.insert(Key("value"));
        let options = InvokeOptions {
            invocation_state: state,
            ..InvokeOptions::default()
        };
        collect(agent.stream_with("Test prompt", options)).await;
        assert_eq!(entries(&seen), ["value"]);
    }

    #[tokio::test]
    async fn middleware_can_short_circuit_the_entire_agent_stream() {
        let model = mock(vec![vec![text("Should not reach")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(AgentStreamStage::wrap(|_, _| short_circuit_result()));
        let events = collect(agent.stream("Test prompt")).await;
        assert!(events.iter().all(is_hook_record));
        assert_eq!(last_result(&events).stop_reason(), StopReason::EndTurn);
        assert!(model.requests().is_empty());
    }

    #[tokio::test]
    async fn multiple_middleware_execute_in_registration_order() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        agent.add_middleware(around::<AgentStreamStage>(&log, "first-before", "first-after"));
        agent.add_middleware(around::<AgentStreamStage>(&log, "second-before", "second-after"));
        collect(agent.stream("Test prompt")).await;
        assert_eq!(
            entries(&log),
            ["first-before", "second-before", "second-after", "first-after"]
        );
    }

    fn dropping(kind: &'static str) -> crate::middleware::Middleware<AgentStreamStage> {
        AgentStreamStage::wrap(move |context, next| {
            Box::pin(next.run(context).filter(move |item| {
                let keep = !matches!(item, Ok(MiddlewareItem::Event(event)) if type_of(event) == kind);
                std::future::ready(keep)
            }))
        })
    }

    #[tokio::test]
    async fn filters_out_specific_event_types() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(dropping("modelStreamUpdateEvent"));
        let events = collect(agent.stream("Test prompt")).await;
        assert!(
            events
                .iter()
                .all(|event| type_of(event) != "modelStreamUpdateEvent")
        );
        assert!(events.iter().any(|event| type_of(event) == "contentBlockEvent"));
    }

    #[tokio::test]
    async fn preserves_the_result_when_filtering_events() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(dropping("contentBlockEvent"));
        let events = collect(agent.stream("Test prompt")).await;
        assert_eq!(last_result(&events).stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn can_suppress_all_events_while_still_returning_the_result() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(AgentStreamStage::wrap(|context, next| {
            Box::pin(
                next.run(context)
                    .filter(|item| std::future::ready(!matches!(item, Ok(MiddlewareItem::Event(_))))),
            )
        }));
        let events = collect(agent.stream("Test prompt")).await;
        assert!(events.iter().all(is_hook_record));
        assert_eq!(last_result(&events).stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn injects_events_before_the_stream() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(AgentStreamStage::wrap(|context, next| {
            Box::pin(
                stream::iter([Ok(MiddlewareItem::Event(synthetic("synthetic")))]).chain(next.run(context)),
            )
        }));
        let events = collect(agent.stream("Test prompt")).await;
        assert_eq!(json_of(&events[1]), json_of(&synthetic("synthetic")));
        assert!(events.len() > 2);
    }

    #[tokio::test]
    async fn injects_events_after_the_stream() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(AgentStreamStage::wrap(|context, next| {
            let mut inner = next.run(context);
            Box::pin(async_stream::stream! {
                let mut result = None;
                while let Some(item) = inner.next().await {
                    match item {
                        Ok(MiddlewareItem::Result(done)) => result = Some(done),
                        other => yield other,
                    }
                }
                yield Ok(MiddlewareItem::Event(synthetic("synthetic")));
                if let Some(result) = result {
                    yield Ok(MiddlewareItem::Result(result));
                }
            })
        }));
        let events = collect(agent.stream("Test prompt")).await;
        let middleware_events: Vec<_> = events.iter().filter(|event| !is_hook_record(event)).collect();
        assert_eq!(
            json_of(middleware_events.last().unwrap()),
            json_of(&synthetic("synthetic"))
        );
    }

    #[tokio::test]
    async fn injects_events_alongside_real_events_via_manual_iteration() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(AgentStreamStage::wrap(|context, next| {
            let mut inner = next.run(context);
            Box::pin(async_stream::stream! {
                let mut injected = false;
                while let Some(item) = inner.next().await {
                    yield item;
                    if !injected {
                        injected = true;
                        yield Ok(MiddlewareItem::Event(synthetic("synthetic")));
                    }
                }
            })
        }));
        let events = collect(agent.stream("Test prompt")).await;
        assert_eq!(json_of(&events[2]), json_of(&synthetic("synthetic")));
        assert_eq!(
            events
                .iter()
                .filter(|event| json_of(event) == json_of(&synthetic("synthetic")))
                .count(),
            1
        );
    }

    #[tokio::test]
    async fn can_yield_events_without_calling_next_pure_synthetic_stream() {
        let model = mock(vec![vec![text("Should not reach")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(AgentStreamStage::wrap(|_, _| {
            let events = stream::iter([
                Ok(MiddlewareItem::Event(synthetic("1"))),
                Ok(MiddlewareItem::Event(synthetic("2"))),
            ]);
            Box::pin(events.chain(short_circuit_result()))
        }));
        let events = collect(agent.stream("Test prompt")).await;
        let middleware_events: Vec<_> = events
            .iter()
            .filter(|event| !is_hook_record(event))
            .map(json_of)
            .collect();
        assert_eq!(
            middleware_events,
            [json_of(&synthetic("1")), json_of(&synthetic("2"))]
        );
    }

    #[tokio::test]
    async fn agent_streams_directly_without_middleware_overhead() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let events = collect(agent.stream("Test prompt")).await;
        assert_eq!(last_result(&events).stop_reason(), StopReason::EndTurn);
        assert!(!events.is_empty());
    }

    #[tokio::test]
    async fn existing_behavior_is_unchanged_when_no_middleware_is_registered() {
        let plain = mock(vec![vec![text("Hello")]]);
        let wrapped = mock(vec![vec![text("Hello")]]);
        let plain_events = collect(agent_with(&plain).stream("Test prompt")).await;
        let mut agent = agent_with(&wrapped);
        agent.add_middleware(AgentStreamStage::wrap(|context, next| next.run(context)));
        let wrapped_events = collect(agent.stream("Test prompt")).await;
        let kinds = |events: &[AgentStreamEvent]| events.iter().map(type_of).collect::<Vec<_>>();
        assert_eq!(kinds(&plain_events), kinds(&wrapped_events));
    }

    #[tokio::test]
    async fn middleware_on_other_stages_does_not_affect_agentstreamstage() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        agent.add_middleware(around::<InvokeModelStage>(&log, "model-before", "model-after"));
        let events = collect(agent.stream("Test prompt")).await;
        assert_eq!(entries(&log), ["model-before", "model-after"]);
        assert_eq!(last_result(&events).stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn beforeinvocationevent_fires_even_when_middleware_short_circuits() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let hook_log = Arc::clone(&log);
        agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&hook_log, "before"));
        agent.add_middleware(AgentStreamStage::wrap(|_, _| short_circuit_result()));
        collect(agent.stream("Test prompt")).await;
        assert_eq!(entries(&log), ["before"]);
    }

    #[tokio::test]
    async fn afterinvocationevent_fires_even_when_middleware_short_circuits() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let hook_log = Arc::clone(&log);
        agent.add_hook(move |_: &mut AfterInvocationEvent<'_>| push(&hook_log, "after"));
        agent.add_middleware(AgentStreamStage::wrap(|_, _| short_circuit_result()));
        collect(agent.stream("Test prompt")).await;
        assert_eq!(entries(&log), ["after"]);
    }

    #[tokio::test]
    async fn beforeinvocationevent_fires_before_middleware_executes() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let hook_log = Arc::clone(&log);
        agent.add_hook(move |_: &mut BeforeInvocationEvent<'_>| push(&hook_log, "beforeInvocation"));
        agent.add_middleware(around::<AgentStreamStage>(&log, "middleware", "middleware-end"));
        collect(agent.stream("Test prompt")).await;
        assert_eq!(entries(&log)[..2], ["beforeInvocation", "middleware"]);
    }

    #[tokio::test]
    async fn afterinvocationevent_fires_after_middleware_completes() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let hook_log = Arc::clone(&log);
        agent.add_hook(move |_: &mut AfterInvocationEvent<'_>| push(&hook_log, "afterInvocation"));
        agent.add_middleware(around::<AgentStreamStage>(
            &log,
            "middleware-start",
            "middleware-end",
        ));
        collect(agent.stream("Test prompt")).await;
        assert_eq!(
            entries(&log),
            ["middleware-start", "middleware-end", "afterInvocation"]
        );
    }

    #[tokio::test]
    async fn replaced_args_replace_the_pass_input() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(AgentStreamStage::wrap(|mut context, next| {
            context.args = InvokeArgs::from("Replaced prompt");
            next.run(context)
        }));
        agent.invoke("Original prompt").await.unwrap();
        assert_eq!(texts(&agent.messages()[0]), ["Replaced prompt"]);
    }

    #[tokio::test]
    async fn a_handler_that_runs_next_twice_at_once_gets_an_error() {
        let model = mock(vec![vec![text("Hello")], vec![text("Again")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(AgentStreamStage::wrap(|context, next| {
            let first = next.run(context.clone());
            let second = next.run(context);
            Box::pin(second.chain(first))
        }));
        let error = agent.invoke("Test").await.unwrap_err();
        assert!(error.to_string().contains("already running"), "{error}");
    }
}

mod execute_tool_stage {
    use super::*;

    #[tokio::test]
    async fn middleware_handler_is_invoked_during_tool_execution() {
        let model = tool_then_done("testTool", json!({}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "executed"));
        let log = Log::default();
        agent.add_middleware(around::<ExecuteToolStage>(
            &log,
            "middleware:before",
            "middleware:after",
        ));
        agent.invoke("Use the tool").await.unwrap();
        assert_eq!(entries(&log), ["middleware:before", "middleware:after"]);
    }

    #[tokio::test]
    async fn middleware_receives_executetoolcontext_with_correct_fields() {
        let model = tool_then_done("testTool", json!({"key": "val"}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "ok"));
        let seen: Arc<Mutex<Option<ExecuteToolContext>>> = Arc::default();
        let slot = Arc::clone(&seen);
        agent.add_middleware(ExecuteToolStage::wrap(move |context, next| {
            *slot.lock().unwrap() = Some(context.clone());
            next.run(context)
        }));
        agent.invoke("Use the tool").await.unwrap();
        let context = seen.lock().unwrap().take().unwrap();
        assert_eq!(context.agent().id(), agent.id());
        assert_eq!(context.tool.as_ref().unwrap().name(), "testTool");
        assert_eq!(context.tool_use.name(), "testTool");
        assert_eq!(context.tool_use.tool_use_id(), "tool-1");
        assert_eq!(context.tool_use.input(), &json!({"key": "val"}));
    }

    #[tokio::test]
    async fn multiple_middleware_execute_in_registration_order() {
        let model = tool_then_done("testTool", json!({}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "ok"));
        let log = Log::default();
        agent.add_middleware(around::<ExecuteToolStage>(&log, "outer-before", "outer-after"));
        agent.add_middleware(around::<ExecuteToolStage>(&log, "inner-before", "inner-after"));
        agent.invoke("Use the tool").await.unwrap();
        assert_eq!(
            entries(&log),
            ["outer-before", "inner-before", "inner-after", "outer-after"]
        );
    }

    fn mocking(text_: &'static str) -> crate::middleware::Middleware<ExecuteToolStage> {
        ExecuteToolStage::wrap(move |context, _| {
            tool_result(text_result(context.tool_use.tool_use_id(), text_))
        })
    }

    fn counting_tool(name: &str, calls: &Arc<Mutex<usize>>) -> crate::test_support::MockTool {
        let calls = Arc::clone(calls);
        mock_tool(name, move |ctx| {
            *calls.lock().unwrap() += 1;
            text_result(ctx.tool_use().tool_use_id(), "real result")
        })
    }

    #[tokio::test]
    async fn returns_mock_result_without_executing_the_real_tool() {
        let model = tool_then_done("testTool", json!({}));
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("testTool", &calls));
        agent.add_middleware(mocking("mocked result"));
        let result = agent.invoke("Use the tool").await.unwrap();
        assert_eq!(*calls.lock().unwrap(), 0);
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn short_circuit_result_is_used_in_the_conversation() {
        let model = mock(vec![
            vec![tool_use("testTool", "tool-1", json!({}))],
            vec![text("Got the mocked data")],
        ]);
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "real"));
        agent.add_middleware(mocking("mocked data"));
        agent.invoke("Use the tool").await.unwrap();
        let block = agent
            .messages()
            .iter()
            .flat_map(Message::content)
            .find_map(|block| match block {
                ContentBlock::ToolResult(result) => Some(result.clone()),
                _ => None,
            })
            .unwrap();
        assert_eq!(block, text_result("tool-1", "mocked data"));
    }

    #[tokio::test]
    async fn modified_input_reaches_the_tool() {
        let model = tool_then_done("testTool", json!({"value": "original"}));
        let received: Arc<Mutex<Option<JsonValue>>> = Arc::default();
        let slot = Arc::clone(&received);
        let tool = mock_tool("testTool", move |ctx| {
            *slot.lock().unwrap() = Some(ctx.tool_use().input().clone());
            text_result(ctx.tool_use().tool_use_id(), "ok")
        });
        let mut agent = agent_with_tool(&model, tool);
        agent.add_middleware(ExecuteToolStage::wrap(|mut context, next| {
            context.tool_use = ToolUse::new(
                context.tool_use.name(),
                context.tool_use.tool_use_id(),
                json!({"value": "transformed"}),
            );
            next.run(context)
        }));
        agent.invoke("Use the tool").await.unwrap();
        assert_eq!(
            received.lock().unwrap().take(),
            Some(json!({"value": "transformed"}))
        );
    }

    #[tokio::test]
    async fn original_context_is_not_mutated() {
        let model = tool_then_done("testTool", json!({"value": "original"}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "ok"));
        let seen: Arc<Mutex<Option<JsonValue>>> = Arc::default();
        let slot = Arc::clone(&seen);
        agent.add_middleware(ExecuteToolStage::wrap(move |context, next| {
            let mut modified = context.clone();
            modified.tool_use = ToolUse::new(
                context.tool_use.name(),
                context.tool_use.tool_use_id(),
                json!({"value": "modified"}),
            );
            *slot.lock().unwrap() = Some(context.tool_use.input().clone());
            next.run(modified)
        }));
        agent.invoke("Use the tool").await.unwrap();
        assert_eq!(seen.lock().unwrap().take(), Some(json!({"value": "original"})));
        assert_eq!(
            agent.messages()[1].content(),
            [tool_use("testTool", "tool-1", json!({"value": "original"}))]
        );
    }

    #[tokio::test]
    async fn beforetoolcallevent_fires_before_middleware_aftertoolcallevent_fires_after() {
        let model = tool_then_done("testTool", json!({}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "executed"));
        let log = Log::default();
        let before = Arc::clone(&log);
        agent.add_hook(move |_: &mut BeforeToolCallEvent| push(&before, "hook:beforeToolCall"));
        let after = Arc::clone(&log);
        agent.add_hook(move |_: &mut AfterToolCallEvent| push(&after, "hook:afterToolCall"));
        agent.add_middleware(around::<ExecuteToolStage>(
            &log,
            "middleware:before",
            "middleware:after",
        ));
        agent.invoke("Use the tool").await.unwrap();
        assert_eq!(
            entries(&log),
            [
                "hook:beforeToolCall",
                "middleware:before",
                "middleware:after",
                "hook:afterToolCall"
            ]
        );
    }

    #[tokio::test]
    async fn hooks_fire_even_when_middleware_short_circuits() {
        let model = tool_then_done("testTool", json!({}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "real"));
        let log = Log::default();
        let before = Arc::clone(&log);
        agent.add_hook(move |_: &mut BeforeToolCallEvent| push(&before, "before"));
        let after = Arc::clone(&log);
        agent.add_hook(move |_: &mut AfterToolCallEvent| push(&after, "after"));
        agent.add_middleware(mocking("mocked"));
        agent.invoke("Use the tool").await.unwrap();
        assert_eq!(entries(&log), ["before", "after"]);
    }

    #[tokio::test]
    async fn aftertoolcallevent_receives_the_middleware_result_when_short_circuited() {
        let model = tool_then_done("testTool", json!({}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "real"));
        let seen: Arc<Mutex<Option<ToolResultBlock>>> = Arc::default();
        let slot = Arc::clone(&seen);
        agent.add_hook(move |event: &mut AfterToolCallEvent| {
            *slot.lock().unwrap() = Some(event.result().clone());
        });
        agent.add_middleware(mocking("from middleware"));
        agent.invoke("Use the tool").await.unwrap();
        assert_eq!(
            seen.lock().unwrap().take(),
            Some(text_result("tool-1", "from middleware"))
        );
    }

    #[tokio::test]
    async fn a_hook_cancelled_call_skips_the_chain() {
        let model = tool_then_done("testTool", json!({}));
        let calls = Arc::new(Mutex::new(0));
        let mut agent = agent_with_tool(&model, counting_tool("testTool", &calls));
        agent.add_hook(|event: &mut BeforeToolCallEvent| event.set_cancel("denied"));
        let log = Log::default();
        agent.add_middleware(around::<ExecuteToolStage>(&log, "before", "after"));
        agent.invoke("Use the tool").await.unwrap();
        assert_eq!(*calls.lock().unwrap(), 0);
        assert!(entries(&log).is_empty());
    }

    /// The sequential executor rethrows; the concurrent one keeps the failure to its tool call
    /// (`concurrent::isolates_unexpected_middleware_failures_to_the_affected_tool`).
    #[tokio::test]
    async fn a_failing_handler_fails_the_invocation() {
        let model = tool_then_done("testTool", json!({}));
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([ok_tool("testTool", "ok")])
            .tool_executor(SequentialToolExecutor::new())
            .printer(false)
            .build()
            .unwrap();
        agent.add_middleware(ExecuteToolStage::wrap(|_, _| {
            Box::pin(stream::iter([Err(Error::other("middleware exploded").into())]))
        }));
        let error = agent.invoke("Use the tool").await.unwrap_err();
        assert_eq!(error.to_string(), "middleware exploded");
    }

    #[tokio::test]
    async fn a_panicking_handler_becomes_an_error_result() {
        let model = tool_then_done("testTool", json!({}));
        let mut agent = agent_with_tool(&model, ok_tool("testTool", "ok"));
        agent.add_middleware(ExecuteToolStage::wrap(|_, _| panic!("handler panicked")));
        let result = agent.invoke("Use the tool").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        let ContentBlock::ToolResult(block) = &agent.messages()[2].content()[0] else {
            panic!("expected a tool result");
        };
        assert_eq!(*block.status(), ToolResultStatus::Error);
        assert_eq!(
            block.content(),
            [crate::types::messages::ToolResultContent::from(
                "handler panicked"
            )]
        );
    }
}

mod use_cases {
    use super::*;

    /// TS `ToolResultCache`.
    #[derive(Default)]
    struct ToolResultCache {
        cache: Arc<Mutex<std::collections::HashMap<String, ToolResultBlock>>>,
    }

    #[async_trait::async_trait]
    impl Plugin for ToolResultCache {
        fn name(&self) -> &str {
            "tool-result-cache"
        }

        async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> crate::Result<()> {
            let cache = Arc::clone(&self.cache);
            agent.add_middleware(ExecuteToolStage::wrap(move |context, next| {
                let key = format!("{}:{}", context.tool_use.name(), context.tool_use.input());
                if let Some(cached) = cache.lock().unwrap().get(&key) {
                    let result = ToolResultBlock::new(
                        context.tool_use.tool_use_id(),
                        cached.status().clone(),
                        cached.content().to_vec(),
                    );
                    return tool_result(result);
                }
                let cache = Arc::clone(&cache);
                Box::pin(next.run(context).map(move |item| {
                    if let Ok(MiddlewareItem::Result(result)) = &item {
                        cache.lock().unwrap().insert(key.clone(), result.result.clone());
                    }
                    item
                }))
            }));
            Ok(())
        }
    }

    #[tokio::test]
    async fn returns_cached_result_on_second_call_skipping_real_execution() {
        let model = mock(vec![
            vec![tool_use("expensiveApi", "call-1", json!({"query": "weather"}))],
            vec![text("First done")],
            vec![tool_use("expensiveApi", "call-2", json!({"query": "weather"}))],
            vec![text("Second done")],
        ]);
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        let tool = mock_tool("expensiveApi", move |ctx| {
            *counter.lock().unwrap() += 1;
            text_result(ctx.tool_use().tool_use_id(), "sunny, 72°F")
        });
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([tool])
            .plugin(ToolResultCache::default())
            .printer(false)
            .build()
            .unwrap();
        agent.invoke("What is the weather?").await.unwrap();
        assert_eq!(*calls.lock().unwrap(), 1);
        agent.invoke("What is the weather?").await.unwrap();
        assert_eq!(*calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn retries_on_transient_error_and_succeeds_on_second_attempt() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| Error::other("ThrottlingException: rate limit exceeded"));
        model.add_turn(vec![text("Success after retry")]);
        let mut agent = agent_with(&model);
        agent.add_middleware(InvokeModelStage::wrap(|context, next| {
            Box::pin(async_stream::stream! {
                for attempt in 0..3 {
                    let mut inner = next.run(context.clone());
                    let mut retry = false;
                    while let Some(item) = inner.next().await {
                        match item {
                            Err(error) if error.to_string().contains("ThrottlingException") && attempt < 2 => {
                                retry = true;
                                break;
                            }
                            other => yield other,
                        }
                    }
                    if !retry {
                        return;
                    }
                }
            })
        }));
        let result = agent.invoke("Hello").await.unwrap();
        assert_eq!(model.requests().len(), 2);
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(result.last_message().content(), [text("Success after retry")]);
    }

    /// TS `StreamFinalTurnOnly`.
    struct StreamFinalTurnOnly;

    #[async_trait::async_trait]
    impl Plugin for StreamFinalTurnOnly {
        fn name(&self) -> &str {
            "stream-final-turn-only"
        }

        async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> crate::Result<()> {
            agent.add_middleware(AgentStreamStage::wrap(|context, next| {
                let mut inner = next.run(context);
                Box::pin(async_stream::stream! {
                    let mut buffer = Vec::new();
                    while let Some(item) = inner.next().await {
                        match item {
                            Ok(MiddlewareItem::Event(event @ (AgentStreamEvent::ContentBlock(_) | AgentStreamEvent::ModelStreamUpdate(_)))) => {
                                buffer.push(event);
                            }
                            Ok(MiddlewareItem::Event(AgentStreamEvent::AfterModelCall(after))) => {
                                let end_turn = after
                                    .stop_data()
                                    .is_some_and(|data| *data.stop_reason() == StopReason::EndTurn);
                                for buffered in std::mem::take(&mut buffer) {
                                    if end_turn {
                                        yield Ok(MiddlewareItem::Event(buffered));
                                    }
                                }
                                yield Ok(MiddlewareItem::Event(AgentStreamEvent::AfterModelCall(after)));
                            }
                            Ok(MiddlewareItem::Result(result)) => {
                                for buffered in std::mem::take(&mut buffer) {
                                    yield Ok(MiddlewareItem::Event(buffered));
                                }
                                yield Ok(MiddlewareItem::Result(result));
                            }
                            other => yield other,
                        }
                    }
                })
            }));
            Ok(())
        }
    }

    fn content_blocks(events: &[AgentStreamEvent]) -> Vec<ContentBlock> {
        events
            .iter()
            .filter_map(|event| match event {
                AgentStreamEvent::ContentBlock(data) => Some(data.content_block().clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn suppresses_content_events_from_intermediate_tool_use_turns_emits_only_final_turn() {
        let model = mock(vec![
            vec![
                text("Let me check that for you"),
                tool_use("lookup", "tool-1", json!({})),
            ],
            vec![text("The answer is 42")],
        ]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .tools([ok_tool("lookup", "42")])
            .plugin(StreamFinalTurnOnly)
            .printer(false)
            .build()
            .unwrap();
        let events = collect(agent.stream("What is the meaning of life?")).await;
        assert_eq!(content_blocks(&events), [text("The answer is 42")]);
        assert_eq!(last_result(&events).stop_reason(), StopReason::EndTurn);
    }

    #[tokio::test]
    async fn passes_through_all_events_when_there_is_only_one_turn_no_tool_use() {
        let model = mock(vec![vec![text("Simple answer")]]);
        let mut agent = Agent::builder()
            .model(Arc::clone(&model))
            .plugin(StreamFinalTurnOnly)
            .printer(false)
            .build()
            .unwrap();
        let events = collect(agent.stream("Hello")).await;
        assert_eq!(content_blocks(&events), [text("Simple answer")]);
        assert_eq!(last_result(&events).stop_reason(), StopReason::EndTurn);
    }
}

mod phases {
    use super::*;

    fn logging_input(log: &Log, name: &'static str) -> crate::middleware::Middleware<InvokeModelStage> {
        let log = Arc::clone(log);
        InvokeModelStage::input(move |context| {
            push(&log, name);
            async move { Ok(context) }
        })
    }

    fn logging_output(log: &Log, name: &'static str) -> crate::middleware::Middleware<InvokeModelStage> {
        let log = Arc::clone(log);
        InvokeModelStage::output(move |result| {
            push(&log, name);
            async move { Ok(result) }
        })
    }

    fn logging_wrap(log: &Log, name: &'static str) -> crate::middleware::Middleware<InvokeModelStage> {
        let log = Arc::clone(log);
        InvokeModelStage::wrap(move |context, next| {
            push(&log, name);
            next.run(context)
        })
    }

    #[tokio::test]
    async fn transforms_context_before_the_model_call() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(InvokeModelStage::input(|mut context| async move {
            context
                .messages
                .push(Message::new(Role::User, vec![text("injected")]));
            Ok(context)
        }));
        let seen: Arc<Mutex<Vec<Message>>> = Arc::default();
        let slot = Arc::clone(&seen);
        agent.add_middleware(InvokeModelStage::wrap(move |context, next| {
            *slot.lock().unwrap() = context.messages.clone();
            next.run(context)
        }));
        agent.invoke("Original").await.unwrap();
        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[1].content()[0], text("injected"));
    }

    #[tokio::test]
    async fn runs_before_wrap_handlers_regardless_of_registration_order() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        agent.add_middleware(logging_wrap(&log, "wrap"));
        agent.add_middleware(logging_input(&log, "input"));
        agent.invoke("Test").await.unwrap();
        assert_eq!(entries(&log), ["input", "wrap"]);
    }

    #[tokio::test]
    async fn multiple_input_handlers_compose_in_registration_order() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        agent.add_middleware(logging_input(&log, "input-1"));
        agent.add_middleware(logging_input(&log, "input-2"));
        agent.invoke("Test").await.unwrap();
        assert_eq!(entries(&log), ["input-1", "input-2"]);
    }

    #[tokio::test]
    async fn transforms_result_after_the_model_call() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let seen: Arc<Mutex<Option<StopReason>>> = Arc::default();
        let slot = Arc::clone(&seen);
        agent.add_middleware(InvokeModelStage::output(move |result| {
            *slot.lock().unwrap() = Some(result.result.stop_reason().clone());
            async move { Ok(result) }
        }));
        agent.invoke("Test").await.unwrap();
        assert_eq!(seen.lock().unwrap().take(), Some(StopReason::EndTurn));
    }

    #[tokio::test]
    async fn transformed_result_message_is_appended_to_agent_messages() {
        let model = mock(vec![vec![text("Original")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(InvokeModelStage::output(|result| async move {
            let stop_reason = result.result.stop_reason().clone();
            Ok(InvokeModelResult::new(StreamAggregatedResult::new(
                assistant("Rewritten by middleware"),
                stop_reason,
            )))
        }));
        agent.invoke("Test").await.unwrap();
        assert_eq!(
            agent.messages().last().unwrap().content(),
            [text("Rewritten by middleware")]
        );
    }

    #[tokio::test]
    async fn runs_after_wrap_handlers_wraps_them() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        agent.add_middleware(logging_output(&log, "output"));
        agent.add_middleware(around::<InvokeModelStage>(&log, "wrap-before", "wrap-after"));
        agent.invoke("Test").await.unwrap();
        assert_eq!(entries(&log), ["wrap-before", "wrap-after", "output"]);
    }

    #[tokio::test]
    async fn changing_stopreason_from_tool_use_to_endturn_prevents_tool_dispatch() {
        let model = mock(vec![
            vec![tool_use("shouldNotRun", "tool-1", json!({}))],
            vec![text("Should not reach here")],
        ]);
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        let tool = mock_tool("shouldNotRun", move |ctx| {
            *counter.lock().unwrap() += 1;
            text_result(ctx.tool_use().tool_use_id(), "ran")
        });
        let mut agent = agent_with_tool(&model, tool);
        agent.add_middleware(InvokeModelStage::output(|result| async move {
            Ok(InvokeModelResult::new(StreamAggregatedResult::new(
                result.result.into_message(),
                StopReason::EndTurn,
            )))
        }));
        let result = agent.invoke("Use the tool").await.unwrap();
        assert_eq!(result.stop_reason(), StopReason::EndTurn);
        assert_eq!(*calls.lock().unwrap(), 0);
    }

    #[tokio::test]
    async fn bare_invokemodelstage_registers_as_wrap_phase_and_executes_between_input_and_output() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        agent.add_middleware(logging_input(&log, "input"));
        agent.add_middleware(logging_wrap(&log, "wrap"));
        agent.add_middleware(logging_output(&log, "output"));
        agent.invoke("Test prompt").await.unwrap();
        assert_eq!(entries(&log), ["input", "wrap", "output"]);
    }

    #[tokio::test]
    async fn input_wrap_output_execution_order() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        agent.add_middleware(logging_output(&log, "output"));
        agent.add_middleware(logging_wrap(&log, "wrap"));
        agent.add_middleware(logging_input(&log, "input"));
        agent.invoke("Test").await.unwrap();
        assert_eq!(entries(&log), ["input", "wrap", "output"]);
    }

    #[tokio::test]
    async fn input_transforms_tool_context_before_execution() {
        let model = tool_then_done("myTool", json!({"x": 1}));
        let received: Arc<Mutex<Option<JsonValue>>> = Arc::default();
        let slot = Arc::clone(&received);
        let tool = mock_tool("myTool", move |ctx| {
            *slot.lock().unwrap() = Some(ctx.tool_use().input().clone());
            text_result(ctx.tool_use().tool_use_id(), "ok")
        });
        let mut agent = agent_with_tool(&model, tool);
        agent.add_middleware(ExecuteToolStage::input(|mut context| async move {
            context.tool_use = ToolUse::new(
                context.tool_use.name(),
                context.tool_use.tool_use_id(),
                json!({"x": 2}),
            );
            Ok(context)
        }));
        agent.invoke("Test").await.unwrap();
        assert_eq!(received.lock().unwrap().take(), Some(json!({"x": 2})));
    }

    #[tokio::test]
    async fn cleanup_removes_input_handler() {
        let model = mock(vec![vec![text("First")], vec![text("Second")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let cleanup = agent.add_middleware(logging_input(&log, "input"));
        agent.invoke("First").await.unwrap();
        cleanup.remove();
        agent.invoke("Second").await.unwrap();
        assert_eq!(entries(&log), ["input"]);
    }

    #[tokio::test]
    async fn cleanup_removes_output_handler() {
        let model = mock(vec![vec![text("First")], vec![text("Second")]]);
        let mut agent = agent_with(&model);
        let log = Log::default();
        let cleanup = agent.add_middleware(logging_output(&log, "output"));
        agent.invoke("First").await.unwrap();
        cleanup.remove();
        agent.invoke("Second").await.unwrap();
        assert_eq!(entries(&log), ["output"]);
    }

    #[tokio::test]
    async fn an_input_handler_error_reaches_aftermodelcallevent() {
        let model = mock(vec![vec![text("Hello")]]);
        let mut agent = agent_with(&model);
        agent.add_middleware(InvokeModelStage::input(|_| async {
            Err(MiddlewareError::from(Error::other("input failed")))
        }));
        let seen = Log::default();
        let slot = Arc::clone(&seen);
        agent.add_hook(move |event: &mut AfterModelCallEvent<'_>| {
            if let Some(error) = event.error() {
                push(&slot, &error.to_string());
            }
        });
        let error = agent.invoke("Test").await.unwrap_err();
        assert_eq!(error.to_string(), "input failed");
        assert_eq!(entries(&seen), ["input failed"]);
        assert!(model.requests().is_empty());
    }
}
