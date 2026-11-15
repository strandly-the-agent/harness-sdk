//! The per-tool pipeline both executors run (TS `tools/executors/executor.ts`).

use std::sync::Arc;

use async_trait::async_trait;
use futures::StreamExt;
use tokio::sync::{Mutex, mpsc};

use crate::agent::invocation_state::InvocationState;
use crate::agent::stream_event::{
    AfterToolCallEventData, AgentStreamEvent, BeforeToolCallEventData, EventError, ToolResultEventData,
    ToolStreamUpdateEventData,
};
use crate::agent::tool_agent::{ToolAgent, ToolCallOutcome, ToolCaller};
use crate::cancel::CancelSignal;
use crate::hooks::events::{AfterToolCallEvent, BeforeToolCallEvent, ToolResultEvent, ToolStreamUpdateEvent};
use crate::hooks::registry::HookRegistry;
use crate::interrupt::Interrupt;
use crate::tools::context::ToolContext;
use crate::tools::tool::{Tool, ToolOutcome, ToolStreamEvent, ToolStreamItem};
use crate::types::json::JsonValue;
use crate::types::messages::ToolResultBlock;
use crate::types::tools::{ToolResultStatus, ToolUse};

/// The text of the result a cancelled call gets.
pub(crate) const TOOL_EXECUTION_CANCELLED: &str = "Tool execution cancelled";

const TOOL_CANCELLED_BY_HOOK: &str = "Tool cancelled by hook";

/// How one tool call ended.
#[derive(Debug)]
pub(crate) enum ToolRun {
    Result(ToolResultBlock),
    Interrupt(Vec<Interrupt>),
}

/// What every call of one tool batch shares: the agent view, the hooks, the invocation state and
/// the cancel signal, plus where the calls' stream records go.
///
/// Per-tool hooks and records go through `hook_turn`, so hooks of concurrent calls never run at
/// the same time and the records arrive in the order the hooks ran, as in TS, where the agent's
/// single task runs them between generator steps.
pub(crate) struct ToolBatch {
    agent: ToolAgent,
    hooks: Arc<HookRegistry>,
    invocation_state: InvocationState,
    cancel_signal: CancelSignal,
    records: Option<mpsc::Sender<AgentStreamEvent>>,
    hook_turn: Mutex<()>,
}

impl ToolBatch {
    pub(crate) fn new(
        agent: ToolAgent,
        hooks: Arc<HookRegistry>,
        invocation_state: InvocationState,
        cancel_signal: CancelSignal,
        records: Option<mpsc::Sender<AgentStreamEvent>>,
    ) -> Arc<Self> {
        Arc::new(Self {
            agent,
            hooks,
            invocation_state,
            cancel_signal,
            records,
            hook_turn: Mutex::new(()),
        })
    }

    pub(crate) fn cancel_signal(&self) -> &CancelSignal {
        &self.cancel_signal
    }

    async fn record(&self, event: AgentStreamEvent) {
        if let Some(records) = &self.records {
            // A closed channel means the invocation is gone; the call has nobody to report to.
            let _ = records.send(event).await;
        }
    }

    /// Fires `ToolResultEvent` for a finished call.
    pub(crate) async fn finish(&self, result: &ToolResultBlock) {
        let _turn = self.hook_turn.lock().await;
        let mut event =
            ToolResultEvent::new(self.agent.clone(), result.clone(), self.invocation_state.clone());
        self.hooks.invoke::<ToolResultEvent>(&mut event).await;
        self.record(AgentStreamEvent::ToolResult(ToolResultEventData::new(
            result.clone(),
        )))
        .await;
    }

    /// Runs one call through the hooks and the tool, retrying while an `AfterToolCallEvent` hook
    /// asks to (TS `executeTool`).
    pub(crate) async fn execute_tool(&self, requested: ToolUse) -> ToolRun {
        let registry_tool = self.agent.tool_registry().get(&requested.name).cloned();
        let tool_use_id = requested.tool_use_id.clone();
        let requested_name = requested.name.clone();
        let mut tool_use = requested;
        loop {
            let before = match self.before_tool_call(&tool_use, registry_tool.clone()).await {
                Ok(before) => before,
                Err(run) => return run,
            };
            tool_use = before.tool_use().clone();
            tool_use.tool_use_id.clone_from(&tool_use_id);
            let tool = match before.selected_tool() {
                Some(tool) => Some(Arc::clone(tool)),
                None if tool_use.name != requested_name => {
                    self.agent.tool_registry().get(&tool_use.name).cloned()
                }
                None => registry_tool.clone(),
            };
            let result = match before.cancel() {
                Some(cancel) => error_result(
                    &tool_use.tool_use_id,
                    cancel.message().unwrap_or(TOOL_CANCELLED_BY_HOOK),
                ),
                None => match self.execute_core(tool.clone(), &tool_use).await {
                    ToolOutcome::Result(result) => with_id(result, &tool_use.tool_use_id),
                    ToolOutcome::Interrupt(interrupt) => return ToolRun::Interrupt(vec![interrupt]),
                },
            };
            let (result, retry) = self.after_tool_call(&tool_use, tool, result).await;
            if retry && !self.cancel_signal.is_cancelled() {
                continue;
            }
            return ToolRun::Result(with_id(result, &tool_use_id));
        }
    }

    async fn before_tool_call(
        &self,
        tool_use: &ToolUse,
        tool: Option<Arc<dyn Tool>>,
    ) -> Result<BeforeToolCallEvent, ToolRun> {
        let _turn = self.hook_turn.lock().await;
        let mut event = BeforeToolCallEvent::new(
            self.agent.clone(),
            tool_use.clone(),
            tool,
            self.invocation_state.clone(),
        );
        self.hooks.invoke::<BeforeToolCallEvent>(&mut event).await;
        let interrupts = match event.take_interrupts() {
            Ok(interrupts) => interrupts,
            Err(error) => {
                return Err(ToolRun::Result(error_result(
                    &tool_use.tool_use_id,
                    &error.to_string(),
                )));
            }
        };
        self.record(AgentStreamEvent::BeforeToolCall(BeforeToolCallEventData::new(
            event.tool_use().clone(),
        )))
        .await;
        if !interrupts.is_empty() {
            return Err(ToolRun::Interrupt(interrupts));
        }
        Ok(event)
    }

    async fn after_tool_call(
        &self,
        tool_use: &ToolUse,
        tool: Option<Arc<dyn Tool>>,
        result: ToolResultBlock,
    ) -> (ToolResultBlock, bool) {
        let _turn = self.hook_turn.lock().await;
        let error = result.shared_error();
        let mut event = AfterToolCallEvent::new(
            self.agent.clone(),
            tool_use.clone(),
            tool,
            result,
            self.invocation_state.clone(),
        );
        if let Some(error) = &error {
            event = event.with_error(Arc::clone(error));
        }
        self.hooks.invoke::<AfterToolCallEvent>(&mut event).await;
        let mut data = AfterToolCallEventData::new(tool_use.clone(), event.result().clone());
        if let Some(error) = event.error() {
            data = data.with_error(EventError::new(error.to_string()));
        }
        self.record(AgentStreamEvent::AfterToolCall(data)).await;
        (event.result().clone(), event.retry())
    }

    /// Runs the tool and forwards its progress events (TS `_executeToolCore`).
    async fn execute_core(&self, tool: Option<Arc<dyn Tool>>, tool_use: &ToolUse) -> ToolOutcome {
        let Some(tool) = tool else {
            return ToolOutcome::Result(error_result(
                &tool_use.tool_use_id,
                &format!("Tool '{}' not found in registry", tool_use.name),
            ));
        };
        let mut context = ToolContext::new(
            tool_use.clone(),
            self.agent.clone(),
            self.invocation_state.clone(),
            self.cancel_signal.clone(),
        );
        if let Some(invocation_context) = self.agent.invocation_context() {
            context = context.with_invocation_context(invocation_context.clone());
        }
        let mut stream = tool.stream(context).until_outcome(tool_use);
        while let Some(item) = stream.next().await {
            match item {
                ToolStreamItem::Event(progress) => self.tool_stream_update(progress).await,
                ToolStreamItem::Outcome(outcome) => return outcome,
            }
        }
        ToolOutcome::Result(error_result(
            &tool_use.tool_use_id,
            &format!("Tool '{}' did not return a result", tool_use.name),
        ))
    }

    async fn tool_stream_update(&self, progress: ToolStreamEvent) {
        let _turn = self.hook_turn.lock().await;
        let mut event = ToolStreamUpdateEvent::new(
            self.agent.clone(),
            progress.clone(),
            self.invocation_state.clone(),
        );
        self.hooks.invoke::<ToolStreamUpdateEvent>(&mut event).await;
        self.record(AgentStreamEvent::ToolStreamUpdate(
            ToolStreamUpdateEventData::new(progress),
        ))
        .await;
    }
}

/// An error result with one text block.
pub(crate) fn error_result(tool_use_id: &str, text: &str) -> ToolResultBlock {
    ToolResultBlock::new(tool_use_id, ToolResultStatus::Error, vec![text.into()])
}

fn with_id(mut result: ToolResultBlock, tool_use_id: &str) -> ToolResultBlock {
    if result.tool_use_id != tool_use_id {
        tool_use_id.clone_into(&mut result.tool_use_id);
    }
    result
}

/// [`ToolAgent::call_tool`]: a registry tool through the full per-tool pipeline, outside any
/// stream.
pub(crate) struct PipelineToolCaller {
    hooks: Arc<HookRegistry>,
    invocation_state: InvocationState,
    cancel_signal: CancelSignal,
}

impl PipelineToolCaller {
    pub(crate) fn new(
        hooks: Arc<HookRegistry>,
        invocation_state: InvocationState,
        cancel_signal: CancelSignal,
    ) -> Arc<Self> {
        Arc::new(Self {
            hooks,
            invocation_state,
            cancel_signal,
        })
    }
}

#[async_trait]
impl ToolCaller for PipelineToolCaller {
    async fn call_tool(&self, agent: &ToolAgent, name: &str, input: JsonValue) -> ToolCallOutcome {
        let tool_use = ToolUse::new(name, format!("tooluse_{}", uuid::Uuid::new_v4()), input);
        if self.cancel_signal.is_cancelled() {
            return ToolCallOutcome::Cancelled(error_result(&tool_use.tool_use_id, TOOL_EXECUTION_CANCELLED));
        }
        let batch = ToolBatch::new(
            agent.clone(),
            Arc::clone(&self.hooks),
            self.invocation_state.clone(),
            self.cancel_signal.clone(),
            None,
        );
        match batch.execute_tool(tool_use).await {
            ToolRun::Result(result) => ToolCallOutcome::Result(result),
            ToolRun::Interrupt(interrupts) => ToolCallOutcome::Interrupt(interrupts),
        }
    }
}
