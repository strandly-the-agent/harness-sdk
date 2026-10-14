//! The built-in stages (`src/middleware/stages.ts`).

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use super::Stage;
use crate::agent::{Agent, InvocationState};
use crate::models::{Model, ModelStreamEvent, StreamAggregatedResult};
use crate::tools::{ToolRef, ToolStreamEvent};
use crate::types::{Message, SystemPrompt, ToolChoice, ToolResultBlock, ToolSpec, ToolUse};

/// The model call: one `Model::stream` plus aggregation.
///
/// Context [`InvokeModelContext`], result `StreamAggregatedResult`, events `ModelStreamEvent`.
#[derive(Clone, Copy, Debug)]
pub struct InvokeModelStage;

impl Stage for InvokeModelStage {
    type Context = InvokeModelContext;
    type Result = StreamAggregatedResult;
    type Event = ModelStreamEvent;

    fn name() -> &'static str {
        "invokeModel"
    }
}

/// Input of one model call. Collections are owned copies; `invocation_state` is shared.
#[non_exhaustive]
pub struct InvokeModelContext {
    /// The agent, as an escape hatch.
    pub agent: Agent,
    /// Model to call; replaceable for this call only.
    pub model: Arc<dyn Model>,
    /// Messages to send (a copy of the conversation).
    pub messages: Vec<Message>,
    /// System prompt for this call.
    pub system_prompt: Option<SystemPrompt>,
    /// Tool specs offered to the model.
    pub tool_specs: Vec<ToolSpec>,
    /// Tool choice for this call.
    pub tool_choice: Option<ToolChoice>,
    /// Invocation state, shared by reference with hooks and tools.
    pub invocation_state: InvocationState,
    /// Estimated input tokens, `None` when estimation failed.
    pub projected_input_tokens: Option<u64>,
}

/// One tool call made by the executor.
///
/// Context [`ExecuteToolContext`], result `ToolResultBlock`, events `ToolStreamEvent`.
#[derive(Clone, Copy, Debug)]
pub struct ExecuteToolStage;

impl Stage for ExecuteToolStage {
    type Context = ExecuteToolContext;
    type Result = ToolResultBlock;
    type Event = ToolStreamEvent;

    fn name() -> &'static str {
        "executeTool"
    }
}

/// Input of one tool call. `tool_use` is an owned copy; `invocation_state` is shared.
#[non_exhaustive]
pub struct ExecuteToolContext {
    /// The agent, as an escape hatch.
    pub agent: Agent,
    /// The resolved tool, or `None` when the name is not in the registry.
    pub tool: Option<ToolRef>,
    /// The tool use to execute.
    pub tool_use: ToolUse,
    /// Invocation state, shared by reference.
    pub invocation_state: InvocationState,
    /// Executor-owned cancellation: observe only, middleware cannot replace it.
    pub(crate) cancel: CancellationToken,
}

impl ExecuteToolContext {
    /// One tool-call context.
    pub(crate) fn new(
        agent: Agent,
        tool: Option<ToolRef>,
        tool_use: ToolUse,
        invocation_state: InvocationState,
        cancel: CancellationToken,
    ) -> Self {
        Self {
            agent,
            tool,
            tool_use,
            invocation_state,
            cancel,
        }
    }

    /// The invocation's cancellation token.
    pub fn cancel(&self) -> &CancellationToken {
        &self.cancel
    }
}

// Interrupts raised from inside the chain (`Error::Interrupt` passthrough) land with the
// interrupts module; this context grows an `interrupt()` seam then.
