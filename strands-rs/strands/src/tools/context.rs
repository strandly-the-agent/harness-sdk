//! What a running tool sees (TS `ToolContext`).

use std::fmt;
use std::sync::PoisonError;

use crate::agent::{InvocationContext, InvocationState, ToolAgent};
use crate::cancel::CancelSignal;
use crate::interrupt::{Interrupt, InterruptSource};
use crate::tools::ToolUse;
use crate::types::{InterruptParams, JsonValue};

/// The context passed to [`Tool::stream`](crate::tools::Tool::stream) (TS `ToolContext`).
///
/// Cheap to clone and `'static`, so a tool can move it into spawned work.
#[derive(Clone)]
pub struct ToolContext {
    pub(crate) tool_use: ToolUse,
    pub(crate) agent: ToolAgent,
    pub(crate) invocation_state: InvocationState,
    pub(crate) cancel_signal: CancelSignal,
    pub(crate) invocation_context: Option<InvocationContext>,
}

impl ToolContext {
    /// The tool call: name, id and input.
    pub fn tool_use(&self) -> &ToolUse {
        &self.tool_use
    }

    /// The agent view.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The caller's invocation state.
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The cancel signal. Long-running tools should stop when it fires.
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.cancel_signal
    }

    /// The invocation context, for passing to nested agents.
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// Asks the caller for input (TS `context.interrupt`).
    ///
    /// On resume this returns the caller's response. Otherwise it returns the [`Interrupt`] to
    /// pause on; return it with `?` so the tool stops and re-runs from the start on resume.
    ///
    /// # Errors
    ///
    /// The interrupt, when it has no response yet.
    #[expect(
        clippy::result_large_err,
        reason = "`?` converts the interrupt into a ToolError"
    )]
    pub fn interrupt(&self, params: impl Into<InterruptParams>) -> Result<JsonValue, Interrupt> {
        let params = params.into();
        let id = format!("tool:{}:{}", self.tool_use.tool_use_id, params.name);
        self.agent
            .interrupt_state()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .interrupt(&id, params, InterruptSource::Tool)
            .map_err(|interrupt| *interrupt)
    }
}

impl fmt::Debug for ToolContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolContext")
            .field("tool_use", &self.tool_use)
            .field("agent", &self.agent)
            .finish_non_exhaustive()
    }
}
