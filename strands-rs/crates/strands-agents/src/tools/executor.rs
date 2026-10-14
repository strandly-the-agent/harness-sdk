//! Tool executors (`strands-ts/src/tools/executors/*.ts`).
//!
//! Turns the model's `ToolUseBlock`s into `ToolResultBlock`s, firing the tool hooks around each
//! call. Interrupts, telemetry, middleware and background tasks are out of scope for this port.

use async_trait::async_trait;
use futures::stream::{FuturesUnordered, StreamExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::agent::{Agent, InvocationState};
use crate::error::{Error, Result};
use crate::hooks::events::{AfterToolCallEvent, BeforeToolCallEvent, ToolResultEvent, ToolStreamUpdateEvent};
use crate::middleware::{ExecuteToolContext, ExecuteToolStage};
use crate::tools::tool::{ToolContext, ToolRef, ToolStreamEvent};
use crate::types::{ToolResultBlock, ToolUse, ToolUseBlock};

/// Text of the result produced when a hook cancels a tool call without a message.
#[doc(hidden)]
pub const HOOK_CANCELLED_MESSAGE: &str = "Tool cancelled by hook";
/// Text of the result produced when the invocation is cancelled before a tool runs.
#[doc(hidden)]
pub const EXECUTION_CANCELLED_MESSAGE: &str = "Tool execution cancelled";

/// One tool-executor invocation.
#[non_exhaustive]
pub struct ExecuteToolsInput {
    /// Agent whose tools are being executed.
    pub agent: Agent,
    /// Tool uses from the assistant message, in model order.
    pub tool_uses: Vec<ToolUseBlock>,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// Cancellation scoped to this invocation.
    pub cancel: CancellationToken,
}

impl ExecuteToolsInput {
    /// One executor invocation over `tool_uses`.
    pub fn new(
        agent: Agent,
        tool_uses: Vec<ToolUseBlock>,
        invocation_state: InvocationState,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            agent,
            tool_uses,
            invocation_state,
            cancel,
        }
    }
}

/// Events an executor emits while running tools.
#[derive(Clone)]
#[non_exhaustive]
pub enum ToolExecutorEvent {
    /// A tool is about to run (hooks have already seen it).
    BeforeToolCall(BeforeToolCallEvent),
    /// A running tool reported progress.
    ToolStreamUpdate(ToolStreamUpdateEvent),
    /// A tool finished (hooks have already seen it).
    AfterToolCall(AfterToolCallEvent),
    /// A tool's final result.
    ToolResult(ToolResultEvent),
}

/// Runs a batch of tool uses, pushing one result per tool use into `results` in model order.
///
/// `events` receives the stream of [`ToolExecutorEvent`]s. `results` is an out-parameter so a
/// caller still sees partial results when execution is cancelled.
#[async_trait]
pub trait ToolExecutor: Send + Sync {
    /// Execute every tool use in `input`.
    async fn execute(
        &self,
        input: &ExecuteToolsInput,
        events: &mpsc::Sender<ToolExecutorEvent>,
        results: &mut Vec<ToolResultBlock>,
    ) -> Result<()>;
}

/// One tool at a time, in model order, re-checking cancellation before each tool.
#[derive(Clone, Copy, Debug, Default)]
pub struct SequentialToolExecutor;

/// All tools at once; per-tool event order is preserved, results stay in model order.
#[derive(Clone, Copy, Debug, Default)]
pub struct ConcurrentToolExecutor;

#[async_trait]
impl ToolExecutor for SequentialToolExecutor {
    async fn execute(
        &self,
        input: &ExecuteToolsInput,
        events: &mpsc::Sender<ToolExecutorEvent>,
        results: &mut Vec<ToolResultBlock>,
    ) -> Result<()> {
        for tool_use in &input.tool_uses {
            if input.cancel.is_cancelled() {
                let result = ToolResultBlock::error(&tool_use.tool_use_id, EXECUTION_CANCELLED_MESSAGE);
                results.push(result.clone());
                send(
                    events,
                    input,
                    ToolExecutorEvent::ToolResult(result_event(input, result)),
                )
                .await;
                continue;
            }
            let result = execute_tool(input, tool_use, events).await?;
            results.push(result.clone());
            send(
                events,
                input,
                ToolExecutorEvent::ToolResult(result_event(input, result)),
            )
            .await;
        }
        Ok(())
    }
}

#[async_trait]
impl ToolExecutor for ConcurrentToolExecutor {
    async fn execute(
        &self,
        input: &ExecuteToolsInput,
        events: &mpsc::Sender<ToolExecutorEvent>,
        results: &mut Vec<ToolResultBlock>,
    ) -> Result<()> {
        let mut pending: FuturesUnordered<_> = input
            .tool_uses
            .iter()
            .enumerate()
            .map(|(index, tool_use)| async move { (index, execute_tool(input, tool_use, events).await) })
            .collect();

        let mut collected: Vec<Option<ToolResultBlock>> = vec![None; input.tool_uses.len()];
        // Interrupts are deferred: siblings run to completion so their results can be reused on
        // resume. Last interrupt wins when several tools in a batch raise.
        let mut interrupt: Option<Error> = None;

        while let Some((index, outcome)) = pending.next().await {
            let tool_use = &input.tool_uses[index];
            let result = match outcome {
                Ok(result) => result,
                Err(Error::Cancelled) => {
                    ToolResultBlock::error(&tool_use.tool_use_id, EXECUTION_CANCELLED_MESSAGE)
                }
                Err(error) if error.is_control_flow() => {
                    interrupt = Some(error);
                    continue;
                }
                Err(error) => ToolResultBlock::error(&tool_use.tool_use_id, error.to_string()),
            };
            collected[index] = Some(result.clone());
            send(
                events,
                input,
                ToolExecutorEvent::ToolResult(result_event(input, result)),
            )
            .await;
        }

        results.extend(collected.into_iter().flatten());
        match interrupt {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// The per-tool pipeline: before hook → tool → after hook (with retry) → id normalization.
async fn execute_tool(
    input: &ExecuteToolsInput,
    tool_use_block: &ToolUseBlock,
    events: &mpsc::Sender<ToolExecutorEvent>,
) -> Result<ToolResultBlock> {
    let registry_tool = input.agent.with_tool_registry(|r| r.get(&tool_use_block.name));

    loop {
        let mut before = BeforeToolCallEvent {
            agent: input.agent.clone(),
            tool_use: ToolUse {
                name: tool_use_block.name.clone(),
                tool_use_id: tool_use_block.tool_use_id.clone(),
                input: tool_use_block.input.clone(),
            },
            tool: registry_tool.clone(),
            invocation_state: input.invocation_state.clone(),
            cancel: crate::hooks::events::Cancel::No,
            selected_tool: None,
        };
        input.agent.hooks().invoke_callbacks(&mut before).await?;
        send(events, input, ToolExecutorEvent::BeforeToolCall(before.clone())).await;

        let mut tool_use = before.tool_use.clone();
        tool_use.tool_use_id = tool_use_block.tool_use_id.clone();

        let effective_tool = match &before.selected_tool {
            Some(tool) => Some(tool.clone()),
            None if tool_use.name != tool_use_block.name => {
                input.agent.with_tool_registry(|r| r.get(&tool_use.name))
            }
            None => registry_tool.clone(),
        };

        // A denying hook wins over background routing (TS `executor.ts:141-163`).
        let (result, error) = match before.cancel.message_or(HOOK_CANCELLED_MESSAGE) {
            Some(message) => (ToolResultBlock::error(&tool_use.tool_use_id, message), None),
            None => {
                if let (Some(background), Some(tool)) =
                    (input.agent.background_tasks(), effective_tool.clone())
                {
                    match background.route_tool_call(&mut tool_use, &tool_use_block.name, tool.name()) {
                        crate::background_tasks::Route::Error(result) => {
                            return Ok(normalize_tool_result_id(*result, &tool_use_block.tool_use_id));
                        }
                        crate::background_tasks::Route::Background => {
                            let result = background.submit_tool_call(
                                &input.agent,
                                tool,
                                tool_use.clone(),
                                input.invocation_state.clone(),
                            );
                            return Ok(normalize_tool_result_id(result, &tool_use_block.tool_use_id));
                        }
                        crate::background_tasks::Route::Foreground => {}
                    }
                }
                run_tool_chain(input, effective_tool.as_ref(), &tool_use, events).await?
            }
        };

        let mut after = AfterToolCallEvent {
            agent: input.agent.clone(),
            tool_use: tool_use.clone(),
            tool: effective_tool,
            result,
            error,
            invocation_state: input.invocation_state.clone(),
            retry: false,
        };
        input.agent.hooks().invoke_callbacks(&mut after).await?;
        send(events, input, ToolExecutorEvent::AfterToolCall(after.clone())).await;

        if after.retry && !input.cancel.is_cancelled() {
            continue;
        }
        return Ok(normalize_tool_result_id(
            after.result,
            &tool_use_block.tool_use_id,
        ));
    }
}

/// Runs one tool call through the [`ExecuteToolStage`] chain; the terminal is [`run_tool`].
///
/// A short-circuiting handler returns its own result and the tool never runs, so the error slot
/// stays empty (`AfterToolCallEvent.error` is `None`, matching TS).
async fn run_tool_chain(
    input: &ExecuteToolsInput,
    tool: Option<&ToolRef>,
    tool_use: &ToolUse,
    events: &mpsc::Sender<ToolExecutorEvent>,
) -> Result<(ToolResultBlock, Option<std::sync::Arc<Error>>)> {
    if !input.agent.middleware().has::<ExecuteToolStage>() {
        return run_tool(input, tool, tool_use, events).await;
    }

    let context = ExecuteToolContext::new(
        input.agent.clone(),
        tool.cloned(),
        tool_use.clone(),
        input.invocation_state.clone(),
        input.cancel.clone(),
    );
    let error_slot: std::sync::Arc<parking_lot::Mutex<Option<std::sync::Arc<Error>>>> =
        std::sync::Arc::new(parking_lot::Mutex::new(None));
    let sink = tool_event_sink(input, events);

    let terminal = {
        let input_agent = input.agent.clone();
        let invocation_state = input.invocation_state.clone();
        let cancel = input.cancel.clone();
        let events = events.clone();
        let error_slot = std::sync::Arc::clone(&error_slot);
        move |context: ExecuteToolContext, _sink: crate::middleware::EventSink<ToolStreamEvent>| {
            let inner = ExecuteToolsInput::new(
                input_agent.clone(),
                Vec::new(),
                invocation_state.clone(),
                cancel.clone(),
            );
            let events = events.clone();
            let error_slot = std::sync::Arc::clone(&error_slot);
            async move {
                let (result, error) =
                    run_tool(&inner, context.tool.as_ref(), &context.tool_use, &events).await?;
                *error_slot.lock() = error;
                Ok(result)
            }
        }
    };

    let result = input
        .agent
        .middleware()
        .invoke::<ExecuteToolStage>(context, &sink, &terminal)
        .await?;
    let error = error_slot.lock().take();
    Ok((result, error))
}

/// Sink that republishes stage events as [`ToolExecutorEvent::ToolStreamUpdate`].
fn tool_event_sink(
    input: &ExecuteToolsInput,
    events: &mpsc::Sender<ToolExecutorEvent>,
) -> crate::middleware::EventSink<ToolStreamEvent> {
    struct Target {
        agent: Agent,
        invocation_state: InvocationState,
        events: mpsc::Sender<ToolExecutorEvent>,
    }

    #[async_trait]
    impl crate::middleware::EventTarget<ToolStreamEvent> for Target {
        async fn send(&self, event: ToolStreamEvent) -> Result<()> {
            let update = ToolStreamUpdateEvent {
                agent: self.agent.clone(),
                event,
                invocation_state: self.invocation_state.clone(),
            };
            let _ = self
                .events
                .send(ToolExecutorEvent::ToolStreamUpdate(update))
                .await;
            Ok(())
        }
    }

    crate::middleware::EventSink::new(std::sync::Arc::new(Target {
        agent: input.agent.clone(),
        invocation_state: input.invocation_state.clone(),
        events: events.clone(),
    }))
}

/// Invokes the tool, forwarding its progress channel as [`ToolExecutorEvent::ToolStreamUpdate`].
async fn run_tool(
    input: &ExecuteToolsInput,
    tool: Option<&ToolRef>,
    tool_use: &ToolUse,
    events: &mpsc::Sender<ToolExecutorEvent>,
) -> Result<(ToolResultBlock, Option<std::sync::Arc<Error>>)> {
    let Some(tool) = tool else {
        return Ok((
            ToolResultBlock::error(
                &tool_use.tool_use_id,
                format!("Tool '{}' not found in registry", tool_use.name),
            ),
            None,
        ));
    };

    let (progress_tx, mut progress_rx) = mpsc::channel(16);
    let ctx = ToolContext::new(
        tool_use.clone(),
        input.agent.clone(),
        input.invocation_state.clone(),
        input.cancel.clone(),
    )
    .with_progress(progress_tx);

    let invoke = tool.invoke(ctx);
    tokio::pin!(invoke);

    let outcome = loop {
        tokio::select! {
            biased;
            Some(data) = progress_rx.recv() => {
                forward_progress(input, events, data).await;
            }
            outcome = &mut invoke => break outcome,
        }
    };
    while let Ok(data) = progress_rx.try_recv() {
        forward_progress(input, events, data).await;
    }

    match outcome {
        Ok(result) => {
            let error = result
                .error
                .clone()
                .map(|e| std::sync::Arc::new(Error::Other(e.to_string().into())));
            Ok((result, error))
        }
        Err(Error::Cancelled) => Err(Error::Cancelled),
        // An interrupt is control flow: turning it into an error result would answer it with a
        // failure and lose the pause.
        Err(error) if error.is_control_flow() => Err(error),
        Err(error) => {
            let message = error.to_string();
            Ok((
                ToolResultBlock::error(&tool_use.tool_use_id, message),
                Some(std::sync::Arc::new(error)),
            ))
        }
    }
}

async fn forward_progress(
    input: &ExecuteToolsInput,
    events: &mpsc::Sender<ToolExecutorEvent>,
    data: crate::types::JsonValue,
) {
    let event = ToolStreamUpdateEvent {
        agent: input.agent.clone(),
        event: ToolStreamEvent { data: Some(data) },
        invocation_state: input.invocation_state.clone(),
    };
    send(events, input, ToolExecutorEvent::ToolStreamUpdate(event)).await;
}

/// Guarantees the model always sees the id it issued, even if a hook rewrote it.
fn normalize_tool_result_id(result: ToolResultBlock, tool_use_id: &str) -> ToolResultBlock {
    if result.tool_use_id == tool_use_id {
        return result;
    }
    ToolResultBlock {
        tool_use_id: tool_use_id.to_owned(),
        ..result
    }
}

fn result_event(input: &ExecuteToolsInput, result: ToolResultBlock) -> ToolResultEvent {
    ToolResultEvent {
        agent: input.agent.clone(),
        result,
        invocation_state: input.invocation_state.clone(),
    }
}

async fn send(
    events: &mpsc::Sender<ToolExecutorEvent>,
    _input: &ExecuteToolsInput,
    event: ToolExecutorEvent,
) {
    let _ = events.send(event).await;
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::agent::test_agent;
    use crate::hooks::events::Cancel;
    use crate::tools::FunctionTool;
    use crate::types::{JsonValue, ToolResultContent, ToolResultStatus};

    fn echo_tool(name: &'static str) -> ToolRef {
        Arc::new(
            FunctionTool::builder(name).description("echo").handler(
                move |input: JsonValue, _ctx: ToolContext| async move { Ok(json!({ "echo": input })) },
            ),
        )
    }

    fn block(name: &str, id: &str) -> ToolUseBlock {
        ToolUseBlock::new(name, id, json!({ "value": 1 }))
    }

    fn input(agent: &Agent, tool_uses: Vec<ToolUseBlock>) -> ExecuteToolsInput {
        ExecuteToolsInput {
            agent: agent.clone(),
            tool_uses,
            invocation_state: InvocationState::default(),
            cancel: CancellationToken::new(),
        }
    }

    async fn run<E: ToolExecutor>(
        executor: &E,
        input: &ExecuteToolsInput,
    ) -> (Result<()>, Vec<ToolResultBlock>, Vec<ToolExecutorEvent>) {
        let (tx, mut rx) = mpsc::channel(256);
        let mut results = Vec::new();
        let outcome = executor.execute(input, &tx, &mut results).await;
        drop(tx);
        let mut events = Vec::new();
        while let Some(event) = rx.recv().await {
            events.push(event);
        }
        (outcome, results, events)
    }

    fn text(result: &ToolResultBlock) -> String {
        result
            .content
            .iter()
            .filter_map(|c| match c {
                ToolResultContent::Text(t) => Some(t.clone()),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn returns_one_result_per_tool_use_in_model_order() {
        let agent = test_agent();
        agent.with_tool_registry(|r| {
            r.add_or_replace([echo_tool("a"), echo_tool("b")]).unwrap();
        });
        let input = input(&agent, vec![block("a", "1"), block("b", "2")]);
        let (outcome, results, _) = run(&SequentialToolExecutor, &input).await;
        assert!(outcome.is_ok());
        assert_eq!(
            results.iter().map(|r| r.tool_use_id.as_str()).collect::<Vec<_>>(),
            ["1", "2"]
        );
    }

    #[tokio::test]
    async fn unknown_tool_produces_the_not_found_error_result() {
        let agent = test_agent();
        let input = input(&agent, vec![block("missing", "1")]);
        let (_, results, events) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(results[0].status, ToolResultStatus::Error);
        assert_eq!(text(&results[0]), "Tool 'missing' not found in registry");
        let after = events
            .iter()
            .find_map(|e| match e {
                ToolExecutorEvent::AfterToolCall(a) => Some(a),
                _ => None,
            })
            .unwrap();
        assert!(after.error.is_none());
    }

    #[tokio::test]
    async fn emits_before_after_and_result_events_per_tool() {
        let agent = test_agent();
        agent.with_tool_registry(|r| r.add(echo_tool("a")).unwrap());
        let input = input(&agent, vec![block("a", "1")]);
        let (_, _, events) = run(&SequentialToolExecutor, &input).await;
        let kinds: Vec<&str> = events
            .iter()
            .map(|e| match e {
                ToolExecutorEvent::BeforeToolCall(_) => "before",
                ToolExecutorEvent::ToolStreamUpdate(_) => "stream",
                ToolExecutorEvent::AfterToolCall(_) => "after",
                ToolExecutorEvent::ToolResult(_) => "result",
            })
            .collect();
        assert_eq!(kinds, ["before", "after", "result"]);
    }

    #[tokio::test]
    async fn before_hook_rename_re_resolves_the_tool() {
        let agent = test_agent();
        agent.with_tool_registry(|r| {
            r.add_or_replace([echo_tool("a"), echo_tool("b")]).unwrap();
        });
        let _ = agent.hooks().add_sync_callback(|e: &mut BeforeToolCallEvent| {
            e.tool_use.name = "b".to_owned();
            Ok(())
        });
        let input = input(&agent, vec![block("a", "1")]);
        let (_, results, events) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(results[0].status, ToolResultStatus::Success);
        let after = events
            .iter()
            .find_map(|e| match e {
                ToolExecutorEvent::AfterToolCall(a) => Some(a),
                _ => None,
            })
            .unwrap();
        assert_eq!(after.tool.as_ref().unwrap().name(), "b");
        assert_eq!(after.tool_use.tool_use_id, "1");
    }

    #[tokio::test]
    async fn selected_tool_overrides_the_registry_lookup() {
        let agent = test_agent();
        agent.with_tool_registry(|r| r.add(echo_tool("a")).unwrap());
        let replacement = echo_tool("replacement");
        let _ = agent
            .hooks()
            .add_sync_callback(move |e: &mut BeforeToolCallEvent| {
                e.selected_tool = Some(replacement.clone());
                Ok(())
            });
        let input = input(&agent, vec![block("a", "1")]);
        let (_, results, events) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(results[0].status, ToolResultStatus::Success);
        let after = events
            .iter()
            .find_map(|e| match e {
                ToolExecutorEvent::AfterToolCall(a) => Some(a),
                _ => None,
            })
            .unwrap();
        assert_eq!(after.tool.as_ref().unwrap().name(), "replacement");
    }

    #[tokio::test]
    async fn hook_cancel_short_circuits_the_tool() {
        let agent = test_agent();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        agent.with_tool_registry(|r| {
            r.add(Arc::new(FunctionTool::builder("a").description("counts").handler(
                move |_input: JsonValue, _ctx: ToolContext| {
                    let seen = seen.clone();
                    async move {
                        seen.fetch_add(1, Ordering::SeqCst);
                        Ok(json!("ran"))
                    }
                },
            )) as ToolRef)
                .unwrap()
        });
        let _ = agent.hooks().add_sync_callback(|e: &mut BeforeToolCallEvent| {
            e.cancel = Cancel::Reason("nope".to_owned());
            Ok(())
        });
        let input = input(&agent, vec![block("a", "1")]);
        let (_, results, _) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(text(&results[0]), "nope");
        assert_eq!(results[0].status, ToolResultStatus::Error);
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn hook_cancel_without_a_message_uses_the_default_text() {
        let agent = test_agent();
        agent.with_tool_registry(|r| r.add(echo_tool("a")).unwrap());
        let _ = agent.hooks().add_sync_callback(|e: &mut BeforeToolCallEvent| {
            e.cancel = Cancel::Yes;
            Ok(())
        });
        let input = input(&agent, vec![block("a", "1")]);
        let (_, results, _) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(text(&results[0]), HOOK_CANCELLED_MESSAGE);
    }

    #[tokio::test]
    async fn after_hook_can_replace_the_result() {
        let agent = test_agent();
        agent.with_tool_registry(|r| r.add(echo_tool("a")).unwrap());
        let _ = agent.hooks().add_sync_callback(|e: &mut AfterToolCallEvent| {
            e.result = ToolResultBlock::error(&e.result.tool_use_id, "replaced");
            Ok(())
        });
        let input = input(&agent, vec![block("a", "1")]);
        let (_, results, _) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(text(&results[0]), "replaced");
    }

    #[tokio::test]
    async fn after_hook_retry_runs_the_tool_again() {
        let agent = test_agent();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        agent.with_tool_registry(|r| {
            r.add(Arc::new(FunctionTool::builder("a").description("counts").handler(
                move |_input: JsonValue, _ctx: ToolContext| {
                    let seen = seen.clone();
                    async move {
                        seen.fetch_add(1, Ordering::SeqCst);
                        Ok(json!("ran"))
                    }
                },
            )) as ToolRef)
                .unwrap()
        });
        let attempts = Arc::new(AtomicUsize::new(0));
        let hook_attempts = attempts.clone();
        let _ = agent
            .hooks()
            .add_sync_callback(move |e: &mut AfterToolCallEvent| {
                e.retry = hook_attempts.fetch_add(1, Ordering::SeqCst) == 0;
                Ok(())
            });
        let input = input(&agent, vec![block("a", "1")]);
        let (_, results, _) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn normalizes_a_result_id_rewritten_by_a_hook() {
        let agent = test_agent();
        agent.with_tool_registry(|r| r.add(echo_tool("a")).unwrap());
        let _ = agent.hooks().add_sync_callback(|e: &mut AfterToolCallEvent| {
            e.result.tool_use_id = "hijacked".to_owned();
            Ok(())
        });
        let input = input(&agent, vec![block("a", "1")]);
        let (_, results, _) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(results[0].tool_use_id, "1");
    }

    #[tokio::test]
    async fn forwards_tool_progress_as_tool_stream_updates() {
        let agent = test_agent();
        agent.with_tool_registry(|r| {
            r.add(Arc::new(
                FunctionTool::builder("a")
                    .description("reports progress")
                    .handler(|_input: JsonValue, ctx: ToolContext| async move {
                        ctx.progress(json!("one")).await;
                        ctx.progress(json!("two")).await;
                        Ok(json!("done"))
                    }),
            ) as ToolRef)
                .unwrap()
        });
        let input = input(&agent, vec![block("a", "1")]);
        let (_, _, events) = run(&SequentialToolExecutor, &input).await;
        let progress: Vec<JsonValue> = events
            .iter()
            .filter_map(|e| match e {
                ToolExecutorEvent::ToolStreamUpdate(u) => u.event.data.clone(),
                _ => None,
            })
            .collect();
        assert_eq!(progress, vec![json!("one"), json!("two")]);
    }

    #[tokio::test]
    async fn a_failing_tool_becomes_an_error_result_with_the_bare_message() {
        struct Failing;
        #[async_trait]
        impl crate::tools::Tool for Failing {
            fn name(&self) -> &str {
                "a"
            }
            fn description(&self) -> &str {
                "fails"
            }
            fn spec(&self) -> &crate::types::ToolSpec {
                unimplemented!("not used in this test")
            }
            async fn invoke(&self, _ctx: ToolContext) -> Result<ToolResultBlock> {
                Err(Error::other("boom"))
            }
        }
        let agent = test_agent();
        agent.with_tool_registry(|r| r.add(Arc::new(Failing) as ToolRef).unwrap());
        let input = input(&agent, vec![block("a", "1")]);
        let (outcome, results, _) = run(&SequentialToolExecutor, &input).await;
        assert!(outcome.is_ok());
        assert_eq!(text(&results[0]), "boom");
        assert_eq!(results[0].status, ToolResultStatus::Error);
    }

    #[tokio::test]
    async fn uses_the_supplied_signal_between_sequential_tool_executions() {
        let agent = test_agent();
        let cancel = CancellationToken::new();
        let tool_cancel = cancel.clone();
        agent.with_tool_registry(|r| {
            r.add_or_replace([
                Arc::new(FunctionTool::builder("a").description("cancels").handler(
                    move |_input: JsonValue, _ctx: ToolContext| {
                        let tool_cancel = tool_cancel.clone();
                        async move {
                            tool_cancel.cancel();
                            Ok(json!("first"))
                        }
                    },
                )) as ToolRef,
                echo_tool("b"),
            ])
            .unwrap()
        });
        let mut input = input(&agent, vec![block("a", "1"), block("b", "2")]);
        input.cancel = cancel;
        let (_, results, _) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(results[0].status, ToolResultStatus::Success);
        assert_eq!(text(&results[1]), EXECUTION_CANCELLED_MESSAGE);
    }

    #[tokio::test]
    async fn keeps_tools_on_the_supplied_execution_signal() {
        let agent = test_agent();
        let cancel = CancellationToken::new();
        cancel.cancel();
        let seen = Arc::new(AtomicUsize::new(0));
        let recorder = seen.clone();
        agent.with_tool_registry(|r| {
            r.add(Arc::new(
                FunctionTool::builder("a")
                    .description("inspects the signal")
                    .handler(move |_input: JsonValue, ctx: ToolContext| {
                        let recorder = recorder.clone();
                        async move {
                            if ctx.cancel.is_cancelled() {
                                recorder.fetch_add(1, Ordering::SeqCst);
                            }
                            Ok(json!("ran"))
                        }
                    }),
            ) as ToolRef)
                .unwrap()
        });
        let mut input = input(&agent, vec![block("a", "1")]);
        input.cancel = cancel;
        let (_, _, _) = run(&ConcurrentToolExecutor, &input).await;
        assert_eq!(seen.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn does_not_retry_after_the_supplied_signal_is_aborted() {
        let agent = test_agent();
        let cancel = CancellationToken::new();
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = calls.clone();
        let tool_cancel = cancel.clone();
        agent.with_tool_registry(|r| {
            r.add(Arc::new(
                FunctionTool::builder("a")
                    .description("cancels then counts")
                    .handler(move |_input: JsonValue, _ctx: ToolContext| {
                        let seen = seen.clone();
                        let tool_cancel = tool_cancel.clone();
                        async move {
                            seen.fetch_add(1, Ordering::SeqCst);
                            tool_cancel.cancel();
                            Ok(json!("ran"))
                        }
                    }),
            ) as ToolRef)
                .unwrap()
        });
        let _ = agent.hooks().add_sync_callback(|e: &mut AfterToolCallEvent| {
            e.retry = true;
            Ok(())
        });
        let mut input = input(&agent, vec![block("a", "1")]);
        input.cancel = cancel;
        let (_, results, _) = run(&SequentialToolExecutor, &input).await;
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(results.len(), 1);
    }

    #[tokio::test]
    async fn concurrent_collects_results_in_input_order() {
        let agent = test_agent();
        agent.with_tool_registry(|r| {
            r.add_or_replace([
                Arc::new(FunctionTool::builder("slow").description("slow").handler(
                    |_input: JsonValue, _ctx: ToolContext| async move {
                        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
                        Ok(json!("slow"))
                    },
                )) as ToolRef,
                echo_tool("fast"),
            ])
            .unwrap()
        });
        let input = input(&agent, vec![block("slow", "1"), block("fast", "2")]);
        let (_, results, events) = run(&ConcurrentToolExecutor, &input).await;
        assert_eq!(
            results.iter().map(|r| r.tool_use_id.as_str()).collect::<Vec<_>>(),
            ["1", "2"]
        );
        let result_order: Vec<String> = events
            .iter()
            .filter_map(|e| match e {
                ToolExecutorEvent::ToolResult(r) => Some(r.result.tool_use_id.clone()),
                _ => None,
            })
            .collect();
        assert_eq!(result_order, ["2", "1"]);
    }

    #[tokio::test]
    async fn concurrent_preserves_per_tool_event_order() {
        let agent = test_agent();
        agent.with_tool_registry(|r| r.add_or_replace([echo_tool("a"), echo_tool("b")]).unwrap());
        let input = input(&agent, vec![block("a", "1"), block("b", "2")]);
        let (_, _, events) = run(&ConcurrentToolExecutor, &input).await;
        for id in ["1", "2"] {
            let kinds: Vec<&str> = events
                .iter()
                .filter_map(|e| match e {
                    ToolExecutorEvent::BeforeToolCall(b) if b.tool_use.tool_use_id == id => Some("before"),
                    ToolExecutorEvent::AfterToolCall(a) if a.tool_use.tool_use_id == id => Some("after"),
                    ToolExecutorEvent::ToolResult(r) if r.result.tool_use_id == id => Some("result"),
                    _ => None,
                })
                .collect();
            assert_eq!(kinds, ["before", "after", "result"], "tool {id}");
        }
    }

    #[tokio::test]
    async fn concurrent_converts_a_tool_cancellation_into_an_error_result() {
        struct Cancelling;
        #[async_trait]
        impl crate::tools::Tool for Cancelling {
            fn name(&self) -> &str {
                "cancelling"
            }
            fn description(&self) -> &str {
                "cancels"
            }
            fn spec(&self) -> &crate::types::ToolSpec {
                unimplemented!("not used in this test")
            }
            async fn invoke(&self, _ctx: ToolContext) -> Result<ToolResultBlock> {
                Err(Error::Cancelled)
            }
        }
        let agent = test_agent();
        agent.with_tool_registry(|r| {
            r.add_or_replace([Arc::new(Cancelling) as ToolRef, echo_tool("b")])
                .unwrap()
        });
        let input = input(&agent, vec![block("cancelling", "1"), block("b", "2")]);
        let (outcome, results, _) = run(&ConcurrentToolExecutor, &input).await;
        assert!(outcome.is_ok());
        assert_eq!(results.len(), 2);
        assert_eq!(results[0].tool_use_id, "1");
        assert_eq!(results[0].status, ToolResultStatus::Error);
        assert!(format!("{:?}", results[0].content).contains(EXECUTION_CANCELLED_MESSAGE));
        assert_eq!(results[1].tool_use_id, "2");
    }
}
