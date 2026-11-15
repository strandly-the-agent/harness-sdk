//! What a tool receives when it runs (TS `ToolContext` in `tools/tool.ts`).

use std::fmt;
use std::sync::Arc;

use crate::agent::invocation_context::InvocationContext;
use crate::agent::invocation_state::InvocationState;
use crate::agent::tool_agent::ToolAgent;
use crate::cancel::CancelSignal;
use crate::interrupt::{Interrupt, InterruptParams, InterruptSource};
use crate::types::json::JsonValue;
use crate::types::tools::ToolUse;

/// Raises a tool interrupt against the agent's interrupt state.
///
/// The agent loop installs one backed by its interrupt state, so a resumed call finds the stored
/// response; agents run as tools install one that rescopes the id.
pub(crate) trait ToolInterrupter: Send + Sync + 'static {
    /// Returns the response for interrupt `id`, or the interrupt to raise when none exists yet.
    #[expect(clippy::result_large_err, reason = "mirrors ToolContext::interrupt")]
    fn interrupt(&self, id: String, params: InterruptParams) -> Result<JsonValue, Interrupt>;
}

/// Everything a tool receives when it runs (TS `ToolContext`).
///
/// It is `'static` and cheap to clone, so a tool can move it into spawned work.
#[derive(Clone)]
pub struct ToolContext {
    tool_use: ToolUse,
    agent: ToolAgent,
    invocation_state: InvocationState,
    cancel_signal: CancelSignal,
    invocation_context: Option<InvocationContext>,
    interrupter: Option<Arc<dyn ToolInterrupter>>,
}

impl ToolContext {
    /// A context for one call of `tool_use`.
    pub(crate) fn new(
        tool_use: ToolUse,
        agent: ToolAgent,
        invocation_state: InvocationState,
        cancel_signal: CancelSignal,
    ) -> Self {
        Self {
            tool_use,
            agent,
            invocation_state,
            cancel_signal,
            invocation_context: None,
            interrupter: None,
        }
    }

    /// Sets the request this call belongs to.
    pub(crate) fn with_invocation_context(mut self, invocation_context: InvocationContext) -> Self {
        self.invocation_context = Some(invocation_context);
        self
    }

    /// Routes [`interrupt`](Self::interrupt) through `interrupter`.
    #[cfg_attr(not(test), expect(dead_code, reason = "built by the tool executors"))]
    pub(crate) fn with_interrupter(mut self, interrupter: Arc<dyn ToolInterrupter>) -> Self {
        self.interrupter = Some(interrupter);
        self
    }

    /// The tool call being run: name, id and input.
    #[must_use]
    pub fn tool_use(&self) -> &ToolUse {
        &self.tool_use
    }

    /// The agent running the tool, as a shared read-mostly view.
    #[must_use]
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The caller's per-invocation values.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// Cancellation for this call. Long-running tools should watch it.
    #[must_use]
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.cancel_signal
    }

    /// The request this call belongs to; pass it to a nested invocation to run it in the same
    /// request.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// Asks the caller for input (TS `interrupt`).
    ///
    /// On resume it returns the caller's response. Otherwise it returns the [`Interrupt`] to raise
    /// under the id `tool:<toolUseId>:<name>`; propagate it with `?`, which converts it into a
    /// [`ToolError`](crate::ToolError) that pauses the invocation. The tool re-runs from the
    /// start on resume.
    ///
    /// ```
    /// use strands::{ToolContext, ToolError};
    ///
    /// fn confirm(ctx: &ToolContext) -> Result<bool, ToolError> {
    ///     let answer = ctx.interrupt("confirm_delete")?;
    ///     Ok(answer == serde_json::json!("yes"))
    /// }
    /// ```
    ///
    /// # Errors
    ///
    /// The interrupt to raise when no response is stored for it yet.
    #[expect(
        clippy::result_large_err,
        reason = "the design's signature; `?` boxes the interrupt into ToolError"
    )]
    pub fn interrupt(&self, params: impl Into<InterruptParams>) -> Result<JsonValue, Interrupt> {
        let params = params.into();
        let id = tool_interrupt_id(&self.tool_use.tool_use_id, &params.name);
        match &self.interrupter {
            Some(interrupter) => interrupter.interrupt(id, params),
            None => fresh_interrupt(id, params),
        }
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

/// The id of an interrupt a tool raises (TS `tool:<toolUseId>:<name>`).
pub(crate) fn tool_interrupt_id(tool_use_id: &str, name: &str) -> String {
    format!("tool:{tool_use_id}:{name}")
}

/// Raises against empty interrupt state: a response given in `params` answers it at once.
#[expect(clippy::result_large_err, reason = "mirrors ToolContext::interrupt")]
fn fresh_interrupt(id: String, params: InterruptParams) -> Result<JsonValue, Interrupt> {
    if let Some(response) = params.response {
        return Ok(response);
    }
    let mut interrupt = Interrupt::new(id, params.name).with_source(InterruptSource::Tool);
    interrupt.reason = params.reason;
    Err(interrupt)
}
