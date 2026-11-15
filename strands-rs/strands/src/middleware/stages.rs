//! The built-in stages and their contexts (TS `middleware/stages.ts`, `middleware/interrupt.ts`).

use std::fmt;
use std::sync::Arc;

use indexmap::IndexMap;

use super::types::MiddlewareStage;
use crate::agent::InvocationContext;
use crate::cancel::CancelSignal;
use crate::interrupt::{Interrupt, InterruptParams, InterruptSource};
use crate::models::model::{Model, StreamAggregatedResult};
use crate::tools::Tool;
use crate::types::agent::{
    AgentResult, AgentStreamEvent, InterruptState, InvocationState, InvokeArgs, InvokeOptions, ToolAgent,
};
use crate::types::json::JsonValue;
use crate::types::messages::{Message, SystemPrompt, ToolResultBlock};
use crate::types::tools::{ToolChoice, ToolSpec, ToolUse};

/// What `interrupt()` returns on a middleware context once the interrupt has a response (TS
/// `MiddlewareInterruptResult`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct MiddlewareInterruptResult {
    response: JsonValue,
}

impl MiddlewareInterruptResult {
    /// The caller's response.
    pub fn response(&self) -> &JsonValue {
        &self.response
    }
}

/// A middleware context's interrupt function (TS `createMiddlewareInterrupt`).
#[derive(Clone)]
pub(crate) struct MiddlewareInterrupt {
    state: InterruptState,
    prefix: String,
    /// The interrupts at the start of the pass, read instead of the live state.
    prior: Option<Arc<IndexMap<String, Interrupt>>>,
}

impl MiddlewareInterrupt {
    pub(crate) fn new(state: InterruptState, prefix: impl Into<String>) -> Self {
        Self {
            state,
            prefix: prefix.into(),
            prior: None,
        }
    }

    /// Reads responses from the interrupts as they are now, not as they are when it is called.
    pub(crate) fn with_snapshot(mut self) -> Self {
        self.prior = Some(Arc::new(self.state.lock().interrupts.clone()));
        self
    }

    #[expect(
        clippy::result_large_err,
        reason = "mirrors `ToolContext::interrupt`, whose error is the interrupt"
    )]
    fn interrupt(&self, params: InterruptParams) -> Result<MiddlewareInterruptResult, Interrupt> {
        let id = format!("{}:{}", self.prefix, params.name);
        let existing = match &self.prior {
            Some(prior) => prior.get(&id).and_then(|interrupt| interrupt.response().cloned()),
            None => self
                .state
                .lock()
                .interrupts
                .get(&id)
                .and_then(|interrupt| interrupt.response().cloned()),
        };
        if let Some(response) = existing.or(params.response) {
            return Ok(MiddlewareInterruptResult { response });
        }
        let mut interrupt = Interrupt::new(id, params.name).with_source(InterruptSource::Middleware);
        if let Some(reason) = params.reason {
            interrupt = interrupt.with_reason(reason);
        }
        Err(interrupt)
    }
}

/// The stage around each model call (TS `InvokeModelStage`, name `invokeModel`): rate-limit,
/// cache or rewrite model input. Its events are the model's stream and content block events.
#[derive(Debug)]
#[non_exhaustive]
pub struct InvokeModelStage;

impl MiddlewareStage for InvokeModelStage {
    type Context = InvokeModelContext;
    type Result = InvokeModelResult;
    type Event = AgentStreamEvent;
    const NAME: &'static str = "invokeModel";
}

/// The stage around each tool call (TS `ExecuteToolStage`, name `executeTool`): add telemetry,
/// validate input or mock results. It runs inside the tool's `BeforeToolCallEvent` and
/// `AfterToolCallEvent`.
#[derive(Debug)]
#[non_exhaustive]
pub struct ExecuteToolStage;

impl MiddlewareStage for ExecuteToolStage {
    type Context = ExecuteToolContext;
    type Result = ExecuteToolResult;
    type Event = AgentStreamEvent;
    const NAME: &'static str = "executeTool";
}

/// The stage around each pass of an invocation (TS `AgentStreamStage`, name `agentStream`):
/// filter, transform or add the events the agent streams. `BeforeInvocationEvent`,
/// `AfterInvocationEvent` and `AgentResultEvent` fire outside it.
///
/// TS marks this stage internal and exports it only from its middleware module; it is exported
/// from [`strands::middleware`](crate::middleware) only.
#[derive(Debug)]
#[non_exhaustive]
pub struct AgentStreamStage;

impl MiddlewareStage for AgentStreamStage {
    type Context = AgentStreamContext;
    type Result = AgentStreamResult;
    type Event = AgentStreamEvent;
    const NAME: &'static str = "agentStream";
}

/// The context of an [`InvokeModelStage`] chain (TS `InvokeModelContext`).
///
/// The collections are copies of the agent's; changing them changes only this call. The agent's
/// model state isn't exposed: the provider writes to a copy the agent commits after the chain.
#[derive(Clone)]
#[non_exhaustive]
pub struct InvokeModelContext {
    agent: ToolAgent,
    invocation_context: Option<InvocationContext>,
    /// The model this call invokes; the agent's model unless replaced.
    pub model: Arc<dyn Model>,
    /// The messages sent to the model.
    pub messages: Vec<Message>,
    /// The system prompt.
    pub system_prompt: Option<SystemPrompt>,
    /// The tool specs offered to the model.
    pub tool_specs: Vec<ToolSpec>,
    /// How the model selects tools.
    pub tool_choice: Option<ToolChoice>,
    /// The invocation's state, shared with hooks, tools and the result.
    pub invocation_state: InvocationState,
    /// The estimated input tokens of the call, when estimation succeeded.
    pub projected_input_tokens: Option<u64>,
    /// How many trailing blocks of the last user message change on every call.
    pub dynamic_trailing_blocks: Option<usize>,
    /// The JSON schema the call must produce (the structured output retry).
    pub structured_output_schema: Option<JsonValue>,
}

impl InvokeModelContext {
    #[expect(
        clippy::too_many_arguments,
        reason = "the context's fields, set once by the loop"
    )]
    pub(crate) fn new(
        agent: ToolAgent,
        model: Arc<dyn Model>,
        messages: Vec<Message>,
        system_prompt: Option<SystemPrompt>,
        tool_specs: Vec<ToolSpec>,
        tool_choice: Option<ToolChoice>,
        invocation_state: InvocationState,
        projected_input_tokens: Option<u64>,
        structured_output_schema: Option<JsonValue>,
    ) -> Self {
        let invocation_context = agent.invocation_context().cloned();
        Self {
            agent,
            invocation_context,
            model,
            messages,
            system_prompt,
            tool_specs,
            tool_choice,
            invocation_state,
            projected_input_tokens,
            dynamic_trailing_blocks: None,
            structured_output_schema,
        }
    }

    /// A view of the agent at the start of the call.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The request this call belongs to.
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }
}

impl fmt::Debug for InvokeModelContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InvokeModelContext")
            .field("model", &self.model.model_id())
            .field("messages", &self.messages.len())
            .field("tool_specs", &self.tool_specs.len())
            .finish_non_exhaustive()
    }
}

/// The result of an [`InvokeModelStage`] chain (TS `InvokeModelResult`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct InvokeModelResult {
    /// The aggregated model response.
    pub result: StreamAggregatedResult,
}

impl InvokeModelResult {
    /// A result carrying `result`, for a handler that answers without the model.
    pub fn new(result: StreamAggregatedResult) -> Self {
        Self { result }
    }
}

/// The context of an [`ExecuteToolStage`] chain (TS `ExecuteToolContext`).
#[derive(Clone)]
#[non_exhaustive]
pub struct ExecuteToolContext {
    agent: ToolAgent,
    cancel_signal: CancelSignal,
    interrupt: MiddlewareInterrupt,
    /// The tool that runs, or `None` when the name resolves to no tool.
    pub tool: Option<Arc<dyn Tool>>,
    /// The call (name, tool use id, input), a copy of the model's.
    pub tool_use: ToolUse,
    /// The invocation's state, shared with hooks, tools and the result.
    pub invocation_state: InvocationState,
}

impl ExecuteToolContext {
    pub(crate) fn new(
        agent: ToolAgent,
        tool: Option<Arc<dyn Tool>>,
        tool_use: ToolUse,
        invocation_state: InvocationState,
        interrupt: MiddlewareInterrupt,
    ) -> Self {
        let cancel_signal = agent.cancel_signal().clone();
        Self {
            agent,
            cancel_signal,
            interrupt,
            tool,
            tool_use,
            invocation_state,
        }
    }

    /// The agent view the tool runs against.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The executor's cancel signal for this call; middleware can observe but not replace it.
    pub fn cancel_signal(&self) -> &CancelSignal {
        &self.cancel_signal
    }

    /// Raises an interrupt with id `middleware:executeTool:<toolUseId>:<name>`, or returns its
    /// response on resume (TS `interrupt`). `?` turns the interrupt into a
    /// [`MiddlewareError`](super::MiddlewareError).
    ///
    /// # Errors
    ///
    /// The interrupt, when it has no response yet.
    #[expect(
        clippy::result_large_err,
        reason = "mirrors `ToolContext::interrupt`, whose error is the interrupt"
    )]
    pub fn interrupt(
        &self,
        params: impl Into<InterruptParams>,
    ) -> Result<MiddlewareInterruptResult, Interrupt> {
        self.interrupt.interrupt(params.into())
    }
}

impl fmt::Debug for ExecuteToolContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ExecuteToolContext")
            .field("tool_use", &self.tool_use)
            .field("tool", &self.tool.as_ref().map(|tool| tool.name().to_owned()))
            .finish_non_exhaustive()
    }
}

/// The result of an [`ExecuteToolStage`] chain (TS `ExecuteToolResult`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ExecuteToolResult {
    /// The tool result.
    pub result: ToolResultBlock,
}

impl ExecuteToolResult {
    /// A result carrying `result`, for a handler that answers without the tool.
    pub fn new(result: ToolResultBlock) -> Self {
        Self { result }
    }
}

/// The context of an [`AgentStreamStage`] chain (TS `AgentStreamContext`).
#[derive(Clone)]
#[non_exhaustive]
pub struct AgentStreamContext {
    agent: ToolAgent,
    interrupt: MiddlewareInterrupt,
    /// The invocation arguments; replacing them replaces the pass's input.
    pub args: InvokeArgs,
    /// The invocation options. Of these, only `structured_output_schema` is read after the chain
    /// starts; the rest apply to the whole invocation.
    pub options: InvokeOptions,
}

impl AgentStreamContext {
    pub(crate) fn new(
        agent: ToolAgent,
        args: InvokeArgs,
        options: InvokeOptions,
        interrupt: MiddlewareInterrupt,
    ) -> Self {
        Self {
            agent,
            interrupt,
            args,
            options,
        }
    }

    /// A view of the agent at the start of the pass.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// Raises an interrupt with id `middleware:agentStream:<name>`, or returns its response on
    /// resume (TS `interrupt`). Responses are read as they were when the pass started, so a
    /// handler that asks again after `next` still finds its answer.
    ///
    /// # Errors
    ///
    /// The interrupt, when it has no response yet.
    #[expect(
        clippy::result_large_err,
        reason = "mirrors `ToolContext::interrupt`, whose error is the interrupt"
    )]
    pub fn interrupt(
        &self,
        params: impl Into<InterruptParams>,
    ) -> Result<MiddlewareInterruptResult, Interrupt> {
        self.interrupt.interrupt(params.into())
    }
}

impl fmt::Debug for AgentStreamContext {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentStreamContext")
            .field("args", &self.args)
            .finish_non_exhaustive()
    }
}

/// The result of an [`AgentStreamStage`] chain (TS `AgentStreamResult`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct AgentStreamResult {
    /// The pass's result.
    pub result: AgentResult,
}

impl AgentStreamResult {
    /// A result carrying `result`, for a handler that answers without running the pass.
    pub fn new(result: AgentResult) -> Self {
        Self { result }
    }
}
