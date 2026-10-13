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
use crate::hooks::events::{
    AfterToolCallEvent, BeforeToolCallEvent, ToolResultEvent, ToolStreamUpdateEvent,
};
use crate::tools::tool::{ToolContext, ToolRef, ToolStreamEvent};
use crate::types::{ToolResultBlock, ToolUse, ToolUseBlock};

/// Text of the result produced when a hook cancels a tool call without a message.
pub const HOOK_CANCELLED_MESSAGE: &str = "Tool cancelled by hook";
/// Text of the result produced when the invocation is cancelled before a tool runs.
pub const EXECUTION_CANCELLED_MESSAGE: &str = "Tool execution cancelled";

/// One tool-executor invocation.
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

/// Events an executor emits while running tools.
#[derive(Clone)]
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
                let result =
                    ToolResultBlock::error(&tool_use.tool_use_id, EXECUTION_CANCELLED_MESSAGE);
                results.push(result.clone());
                send(events, input, ToolExecutorEvent::ToolResult(result_event(input, result))).await;
                continue;
            }
            let result = execute_tool(input, tool_use, events).await?;
            results.push(result.clone());
            send(events, input, ToolExecutorEvent::ToolResult(result_event(input, result))).await;
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
            .map(|(index, tool_use)| async move {
                (index, execute_tool(input, tool_use, events).await)
            })
            .collect();

        let mut collected: Vec<Option<ToolResultBlock>> = vec![None; input.tool_uses.len()];
        let mut cancelled: Option<Error> = None;

        while let Some((index, outcome)) = pending.next().await {
            let tool_use = &input.tool_uses[index];
            let result = match outcome {
                Ok(result) => result,
                Err(Error::Cancelled) => {
                    cancelled = Some(Error::Cancelled);
                    continue;
                }
                Err(error) => ToolResultBlock::error(&tool_use.tool_use_id, error.to_string()),
            };
            collected[index] = Some(result.clone());
            send(events, input, ToolExecutorEvent::ToolResult(result_event(input, result))).await;
        }

        results.extend(collected.into_iter().flatten());
        match cancelled {
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
        send(
            events,
            input,
            ToolExecutorEvent::BeforeToolCall(before.clone()),
        )
        .await;

        let mut tool_use = before.tool_use.clone();
        tool_use.tool_use_id = tool_use_block.tool_use_id.clone();

        let effective_tool = match &before.selected_tool {
            Some(tool) => Some(tool.clone()),
            None if tool_use.name != tool_use_block.name => {
                input.agent.with_tool_registry(|r| r.get(&tool_use.name))
            }
            None => registry_tool.clone(),
        };

        let (result, error) = match before.cancel.message_or(HOOK_CANCELLED_MESSAGE) {
            Some(message) => (
                ToolResultBlock::error(&tool_use.tool_use_id, message),
                None,
            ),
            None => run_tool(input, effective_tool.as_ref(), &tool_use, events).await?,
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
