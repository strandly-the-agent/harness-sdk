//! Hook events (TS `hooks/events.ts`).
//!
//! Sequential events lend the agent mutably (`event.agent_mut()`); per-tool events
//! ([`BeforeToolCallEvent`], [`AfterToolCallEvent`], [`ToolStreamUpdateEvent`],
//! [`ToolResultEvent`]) are `'static` and hold a [`ToolAgent`]. Fields are read through getters;
//! only the fields TS lets hooks write have setters.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::Error;
use crate::interrupt::{Interrupt, InterruptParams, InterruptSource};
use crate::models::model::Model;
use crate::models::streaming::ModelStreamEvent;
use crate::tools::tool::{Tool, ToolStreamEvent};
use crate::types::agent::{AgentResult, InvocationState, InvokeArgs, LocalAgent, ToolAgent};
use crate::types::json::JsonValue;
use crate::types::messages::{ContentBlock, Message, StopReason, ToolResultBlock};
use crate::types::tools::ToolUse;

type SharedError = Arc<dyn std::error::Error + Send + Sync + 'static>;

/// Redaction a guardrail requested for the user's input (TS `Redaction`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct Redaction {
    user_message: String,
}

impl Redaction {
    /// A redaction replacing the last user message with `user_message`.
    pub fn new(user_message: impl Into<String>) -> Self {
        Self {
            user_message: user_message.into(),
        }
    }

    /// The text that replaces the last user message.
    pub fn user_message(&self) -> &str {
        &self.user_message
    }
}

/// A hook's request to cancel (TS `cancel: true | string`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cancel {
    /// Cancel with the default message.
    Requested,
    /// Cancel with this message.
    Message(String),
}

impl Cancel {
    /// The custom message, if any.
    pub fn message(&self) -> Option<&str> {
        match self {
            Self::Requested => None,
            Self::Message(message) => Some(message),
        }
    }
}

impl From<&str> for Cancel {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

impl From<String> for Cancel {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

/// A hook's request to end the turn after tools (TS `endTurn: true | string | ContentBlock[]`).
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum EndTurn {
    /// End the turn with no final message content.
    Requested,
    /// End the turn with this text as the final message.
    Message(String),
    /// End the turn with these blocks as the final message.
    Content(Vec<ContentBlock>),
}

impl From<&str> for EndTurn {
    fn from(message: &str) -> Self {
        Self::Message(message.to_owned())
    }
}

impl From<String> for EndTurn {
    fn from(message: String) -> Self {
        Self::Message(message)
    }
}

impl From<Vec<ContentBlock>> for EndTurn {
    fn from(content: Vec<ContentBlock>) -> Self {
        Self::Content(content)
    }
}

/// What the model produced in a successful call (TS `ModelStopData`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ModelStopData {
    message: Message,
    stop_reason: StopReason,
    #[serde(skip_serializing_if = "Option::is_none")]
    redaction: Option<Redaction>,
}

impl ModelStopData {
    /// Stop data for `message`.
    pub fn new(message: Message, stop_reason: StopReason) -> Self {
        Self {
            message,
            stop_reason,
            redaction: None,
        }
    }

    /// Adds a guardrail redaction.
    #[must_use]
    pub fn with_redaction(mut self, redaction: Redaction) -> Self {
        self.redaction = Some(redaction);
        self
    }

    /// The assistant message.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Why the model stopped.
    pub fn stop_reason(&self) -> &StopReason {
        &self.stop_reason
    }

    /// The guardrail redaction, if any.
    pub fn redaction(&self) -> Option<&Redaction> {
        self.redaction.as_ref()
    }
}

macro_rules! sequential_accessors {
    () => {
        /// The agent.
        pub fn agent(&self) -> &LocalAgent {
            self.agent
        }

        /// The agent, mutably.
        pub fn agent_mut(&mut self) -> &mut LocalAgent {
            self.agent
        }

        /// The request this event belongs to; pass it as
        /// [`InvokeOptions::invocation_context`](crate::InvokeOptions::invocation_context) to
        /// join it. `None` outside an invocation.
        pub fn invocation_context(&self) -> Option<&crate::agent::InvocationContext> {
            self.agent.invocation_context()
        }
    };
}

macro_rules! invocation_state_accessor {
    () => {
        /// The caller's per-invocation values.
        pub fn invocation_state(&self) -> &InvocationState {
            &self.invocation_state
        }
    };
}

/// Fired once when the agent finishes initializing (TS `InitializedEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct InitializedEvent<'a> {
    agent: &'a mut LocalAgent,
}

impl<'a> InitializedEvent<'a> {
    pub(crate) fn new(agent: &'a mut LocalAgent) -> Self {
        Self { agent }
    }

    sequential_accessors!();
}

/// Fired at the start of an invocation (TS `BeforeInvocationEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct BeforeInvocationEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    messages: Vec<Message>,
    cancel: Option<Cancel>,
}

impl<'a> BeforeInvocationEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        messages: Vec<Message>,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            messages,
            cancel: None,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The input messages for this invocation.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The input messages, mutably.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.messages
    }

    /// Replaces the input messages.
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// The requested cancel, if any.
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels the invocation before the model is called.
    pub fn set_cancel(&mut self, cancel: impl Into<Cancel>) {
        self.cancel = Some(cancel.into());
    }

    /// Withdraws a requested cancel.
    pub fn clear_cancel(&mut self) {
        self.cancel = None;
    }

    pub(crate) fn into_parts(self) -> (Vec<Message>, Option<Cancel>) {
        (self.messages, self.cancel)
    }
}

/// Fired at the end of an invocation (TS `AfterInvocationEvent`). Callbacks run in reverse.
#[derive(Debug)]
#[non_exhaustive]
pub struct AfterInvocationEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    resume: Option<InvokeArgs>,
}

impl<'a> AfterInvocationEvent<'a> {
    pub(crate) fn new(agent: &'a mut LocalAgent, invocation_state: InvocationState) -> Self {
        Self {
            agent,
            invocation_state,
            resume: None,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The input to re-enter the loop with, if a hook set one.
    pub fn resume(&self) -> Option<&InvokeArgs> {
        self.resume.as_ref()
    }

    /// Re-enters the loop with `args` once the invocation ends. Ignored on error or when a limit
    /// stopped the invocation.
    pub fn set_resume(&mut self, args: impl Into<InvokeArgs>) {
        self.resume = Some(args.into());
    }

    /// Withdraws a requested resume.
    pub fn clear_resume(&mut self) {
        self.resume = None;
    }

    #[allow(dead_code, reason = "read by the loop's resume handling")]
    pub(crate) fn take_resume(&mut self) -> Option<InvokeArgs> {
        self.resume.take()
    }
}

/// Fired when a message is appended to the history (TS `MessageAddedEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct MessageAddedEvent<'a> {
    agent: &'a mut LocalAgent,
    message: Message,
    invocation_state: InvocationState,
}

impl<'a> MessageAddedEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        message: Message,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            message,
            invocation_state,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The appended message.
    pub fn message(&self) -> &Message {
        &self.message
    }
}

/// Fired before a tool runs (TS `BeforeToolCallEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct BeforeToolCallEvent {
    agent: ToolAgent,
    tool_use: ToolUse,
    tool: Option<Arc<dyn Tool>>,
    invocation_state: InvocationState,
    cancel: Option<Cancel>,
    selected_tool: Option<Arc<dyn Tool>>,
    interrupts: Vec<Interrupt>,
}

impl BeforeToolCallEvent {
    pub(crate) fn new(
        agent: ToolAgent,
        tool_use: ToolUse,
        tool: Option<Arc<dyn Tool>>,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            tool_use,
            tool,
            invocation_state,
            cancel: None,
            selected_tool: None,
            interrupts: Vec::new(),
        }
    }

    invocation_state_accessor!();

    /// The agent view for tools.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The request this event belongs to; `None` for a direct tool call.
    pub fn invocation_context(&self) -> Option<&crate::agent::InvocationContext> {
        self.agent.invocation_context()
    }

    /// The tool use about to run.
    pub fn tool_use(&self) -> &ToolUse {
        &self.tool_use
    }

    /// Replaces the tool use (for example to rewrite its input).
    pub fn set_tool_use(&mut self, tool_use: ToolUse) {
        self.tool_use = tool_use;
    }

    /// The registry tool matching the tool use, if any.
    pub fn tool(&self) -> Option<&Arc<dyn Tool>> {
        self.tool.as_ref()
    }

    /// The requested cancel, if any.
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Skips the tool; the model sees an error result with the cancel message.
    pub fn set_cancel(&mut self, cancel: impl Into<Cancel>) {
        self.cancel = Some(cancel.into());
    }

    /// Withdraws a requested cancel.
    pub fn clear_cancel(&mut self) {
        self.cancel = None;
    }

    /// The tool a hook selected to run instead of [`Self::tool`].
    pub fn selected_tool(&self) -> Option<&Arc<dyn Tool>> {
        self.selected_tool.as_ref()
    }

    /// Runs `tool` instead of the registry tool.
    pub fn set_selected_tool(&mut self, tool: Arc<dyn Tool>) {
        self.selected_tool = Some(tool);
    }

    /// Asks the user for input before the tool runs (TS `interrupt`).
    ///
    /// Returns the stored response on resume. Otherwise records the interrupt, returns `None`
    /// and the tool doesn't run; the invocation stops with the interrupt once all callbacks have
    /// run. The id is `hook:beforeToolCall:<toolUseId>:<name>`.
    pub fn interrupt(&mut self, params: InterruptParams) -> Option<JsonValue> {
        let id = format!(
            "hook:beforeToolCall:{}:{}",
            self.tool_use.tool_use_id(),
            params.name
        );
        let result = self
            .agent
            .interrupt_state()
            .interrupt(id, params, InterruptSource::Hook);
        collect(&mut self.interrupts, result)
    }

    pub(crate) fn collected_interrupts(&self) -> &[Interrupt] {
        &self.interrupts
    }

    pub(crate) fn take_interrupts(&mut self) -> Vec<Interrupt> {
        std::mem::take(&mut self.interrupts)
    }
}

fn collect(interrupts: &mut Vec<Interrupt>, result: Result<JsonValue, Interrupt>) -> Option<JsonValue> {
    match result {
        Ok(response) => Some(response),
        Err(interrupt) => {
            interrupts.push(interrupt);
            None
        }
    }
}

/// Fired after a tool runs (TS `AfterToolCallEvent`). Callbacks run in reverse.
#[derive(Debug)]
#[non_exhaustive]
pub struct AfterToolCallEvent {
    agent: ToolAgent,
    tool_use: ToolUse,
    tool: Option<Arc<dyn Tool>>,
    result: ToolResultBlock,
    error: Option<SharedError>,
    invocation_state: InvocationState,
    retry: bool,
}

impl AfterToolCallEvent {
    pub(crate) fn new(
        agent: ToolAgent,
        tool_use: ToolUse,
        tool: Option<Arc<dyn Tool>>,
        result: ToolResultBlock,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            tool_use,
            tool,
            result,
            error: None,
            invocation_state,
            retry: false,
        }
    }

    pub(crate) fn with_error(mut self, error: Option<SharedError>) -> Self {
        self.error = error;
        self
    }

    invocation_state_accessor!();

    /// The agent view for tools.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The request this event belongs to; `None` for a direct tool call.
    pub fn invocation_context(&self) -> Option<&crate::agent::InvocationContext> {
        self.agent.invocation_context()
    }

    /// The tool use that ran.
    pub fn tool_use(&self) -> &ToolUse {
        &self.tool_use
    }

    /// The tool that ran, if one was found.
    pub fn tool(&self) -> Option<&Arc<dyn Tool>> {
        self.tool.as_ref()
    }

    /// The tool result.
    pub fn result(&self) -> &ToolResultBlock {
        &self.result
    }

    /// Replaces the result the model sees.
    pub fn set_result(&mut self, result: ToolResultBlock) {
        self.result = result;
    }

    /// The error the tool failed with, if any.
    pub fn error(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.error.as_deref()
    }

    /// Whether a hook asked to run the tool again.
    pub fn retry(&self) -> bool {
        self.retry
    }

    /// Asks to run the tool again with the same input.
    pub fn set_retry(&mut self, retry: bool) {
        self.retry = retry;
    }

    #[allow(dead_code, reason = "read by direct tool calls")]
    pub(crate) fn into_result(self) -> (ToolResultBlock, bool) {
        (self.result, self.retry)
    }
}

/// Fired before each model call (TS `BeforeModelCallEvent`).
///
/// Request rewriting goes through middleware, not this event.
#[derive(Debug)]
#[non_exhaustive]
pub struct BeforeModelCallEvent<'a> {
    agent: &'a mut LocalAgent,
    model: Arc<dyn Model>,
    invocation_state: InvocationState,
    cancel: Option<Cancel>,
    projected_input_tokens: Option<u64>,
}

impl<'a> BeforeModelCallEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        model: Arc<dyn Model>,
        invocation_state: InvocationState,
        projected_input_tokens: Option<u64>,
    ) -> Self {
        Self {
            agent,
            model,
            invocation_state,
            cancel: None,
            projected_input_tokens,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The model about to be called.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// The projected input tokens of the request, when known.
    pub fn projected_input_tokens(&self) -> Option<u64> {
        self.projected_input_tokens
    }

    /// The requested cancel, if any.
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels the invocation instead of calling the model.
    pub fn set_cancel(&mut self, cancel: impl Into<Cancel>) {
        self.cancel = Some(cancel.into());
    }

    /// Withdraws a requested cancel.
    pub fn clear_cancel(&mut self) {
        self.cancel = None;
    }

    pub(crate) fn take_cancel(&mut self) -> Option<Cancel> {
        self.cancel.take()
    }
}

/// Fired after each model call, successful or not (TS `AfterModelCallEvent`). Callbacks run in
/// reverse.
#[derive(Debug)]
#[non_exhaustive]
pub struct AfterModelCallEvent<'a> {
    agent: &'a mut LocalAgent,
    model: Arc<dyn Model>,
    invocation_state: InvocationState,
    attempt_count: u32,
    stop_data: Option<ModelStopData>,
    error: Option<&'a Error>,
    retry: bool,
}

impl<'a> AfterModelCallEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        model: Arc<dyn Model>,
        invocation_state: InvocationState,
        attempt_count: u32,
    ) -> Self {
        Self {
            agent,
            model,
            invocation_state,
            attempt_count,
            stop_data: None,
            error: None,
            retry: false,
        }
    }

    pub(crate) fn with_stop_data(mut self, stop_data: ModelStopData) -> Self {
        self.stop_data = Some(stop_data);
        self
    }

    pub(crate) fn with_error(mut self, error: &'a Error) -> Self {
        self.error = Some(error);
        self
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The model that was called.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// The 1-based attempt number for this turn; resets when the model changes.
    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// The model output, when the call succeeded.
    pub fn stop_data(&self) -> Option<&ModelStopData> {
        self.stop_data.as_ref()
    }

    /// The error, when the call failed.
    pub fn error(&self) -> Option<&'a Error> {
        self.error
    }

    /// Whether a hook asked to call the model again.
    pub fn retry(&self) -> bool {
        self.retry
    }

    /// Asks to discard this call's output and call the model again.
    pub fn set_retry(&mut self, retry: bool) {
        self.retry = retry;
    }
}

/// Fired for every model stream event (TS `ModelStreamUpdateEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct ModelStreamUpdateEvent<'a> {
    agent: &'a mut LocalAgent,
    event: ModelStreamEvent,
    invocation_state: InvocationState,
}

impl<'a> ModelStreamUpdateEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        event: ModelStreamEvent,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            event,
            invocation_state,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The model stream event.
    pub fn event(&self) -> &ModelStreamEvent {
        &self.event
    }
}

/// Fired when the model completes a content block (TS `ContentBlockEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct ContentBlockEvent<'a> {
    agent: &'a mut LocalAgent,
    content_block: ContentBlock,
    invocation_state: InvocationState,
}

impl<'a> ContentBlockEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        content_block: ContentBlock,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            content_block,
            invocation_state,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The completed block.
    pub fn content_block(&self) -> &ContentBlock {
        &self.content_block
    }
}

/// Fired when the model completes its message (TS `ModelMessageEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct ModelMessageEvent<'a> {
    agent: &'a mut LocalAgent,
    message: Message,
    stop_reason: StopReason,
    invocation_state: InvocationState,
}

impl<'a> ModelMessageEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        message: Message,
        stop_reason: StopReason,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            message,
            stop_reason,
            invocation_state,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The assistant message.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Why the model stopped.
    pub fn stop_reason(&self) -> &StopReason {
        &self.stop_reason
    }
}

/// Fired when a tool result is final (TS `ToolResultEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct ToolResultEvent {
    agent: ToolAgent,
    result: ToolResultBlock,
    invocation_state: InvocationState,
}

impl ToolResultEvent {
    pub(crate) fn new(agent: ToolAgent, result: ToolResultBlock, invocation_state: InvocationState) -> Self {
        Self {
            agent,
            result,
            invocation_state,
        }
    }

    invocation_state_accessor!();

    /// The agent view for tools.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The request this event belongs to; `None` for a direct tool call.
    pub fn invocation_context(&self) -> Option<&crate::agent::InvocationContext> {
        self.agent.invocation_context()
    }

    /// The tool result.
    pub fn result(&self) -> &ToolResultBlock {
        &self.result
    }
}

/// Fired for every progress event a tool emits (TS `ToolStreamUpdateEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct ToolStreamUpdateEvent {
    agent: ToolAgent,
    event: ToolStreamEvent,
    invocation_state: InvocationState,
}

impl ToolStreamUpdateEvent {
    pub(crate) fn new(agent: ToolAgent, event: ToolStreamEvent, invocation_state: InvocationState) -> Self {
        Self {
            agent,
            event,
            invocation_state,
        }
    }

    invocation_state_accessor!();

    /// The agent view for tools.
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The request this event belongs to; `None` for a direct tool call.
    pub fn invocation_context(&self) -> Option<&crate::agent::InvocationContext> {
        self.agent.invocation_context()
    }

    /// The tool's progress event.
    pub fn event(&self) -> &ToolStreamEvent {
        &self.event
    }
}

/// Fired with the invocation's result (TS `AgentResultEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct AgentResultEvent<'a> {
    agent: &'a mut LocalAgent,
    result: AgentResult,
    invocation_state: InvocationState,
}

impl<'a> AgentResultEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        result: AgentResult,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            result,
            invocation_state,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The result.
    pub fn result(&self) -> &AgentResult {
        &self.result
    }
}

/// Fired for each interrupt that stops an invocation (TS `InterruptEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct InterruptEvent<'a> {
    agent: &'a mut LocalAgent,
    interrupt: Interrupt,
    invocation_state: InvocationState,
}

impl<'a> InterruptEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        interrupt: Interrupt,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            interrupt,
            invocation_state,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The interrupt.
    pub fn interrupt(&self) -> &Interrupt {
        &self.interrupt
    }
}

/// Fired before a batch of tools runs (TS `BeforeToolsEvent`).
#[derive(Debug)]
#[non_exhaustive]
pub struct BeforeToolsEvent<'a> {
    agent: &'a mut LocalAgent,
    message: Message,
    invocation_state: InvocationState,
    cancel: Option<Cancel>,
    interrupts: Vec<Interrupt>,
}

impl<'a> BeforeToolsEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        message: Message,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            message,
            invocation_state,
            cancel: None,
            interrupts: Vec::new(),
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The assistant message whose tool uses are about to run.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// The requested cancel, if any.
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels the batch; every tool gets an error result with the cancel message.
    pub fn set_cancel(&mut self, cancel: impl Into<Cancel>) {
        self.cancel = Some(cancel.into());
    }

    /// Withdraws a requested cancel.
    pub fn clear_cancel(&mut self) {
        self.cancel = None;
    }

    /// Asks the user for input before the batch runs (TS `interrupt`).
    ///
    /// Returns the stored response on resume; otherwise records the interrupt and returns
    /// `None`. The id is `hook:beforeTools:<name>`.
    pub fn interrupt(&mut self, params: InterruptParams) -> Option<JsonValue> {
        let id = format!("hook:beforeTools:{}", params.name);
        let result = self
            .agent
            .interrupt_state()
            .interrupt(id, params, InterruptSource::Hook);
        collect(&mut self.interrupts, result)
    }

    pub(crate) fn collected_interrupts(&self) -> &[Interrupt] {
        &self.interrupts
    }

    pub(crate) fn take_interrupts(&mut self) -> Vec<Interrupt> {
        std::mem::take(&mut self.interrupts)
    }

    pub(crate) fn take_cancel(&mut self) -> Option<Cancel> {
        self.cancel.take()
    }
}

/// Fired after a batch of tools ran (TS `AfterToolsEvent`). Callbacks run in reverse.
#[derive(Debug)]
#[non_exhaustive]
pub struct AfterToolsEvent<'a> {
    agent: &'a mut LocalAgent,
    message: Message,
    invocation_state: InvocationState,
    end_turn: Option<EndTurn>,
}

impl<'a> AfterToolsEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        message: Message,
        invocation_state: InvocationState,
    ) -> Self {
        Self {
            agent,
            message,
            invocation_state,
            end_turn: None,
        }
    }

    sequential_accessors!();
    invocation_state_accessor!();

    /// The user message holding the batch's tool results.
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// The requested end of turn, if any.
    pub fn end_turn(&self) -> Option<&EndTurn> {
        self.end_turn.as_ref()
    }

    /// Ends the turn instead of calling the model with the tool results.
    pub fn set_end_turn(&mut self, end_turn: impl Into<EndTurn>) {
        self.end_turn = Some(end_turn.into());
    }

    /// Withdraws a requested end of turn.
    pub fn clear_end_turn(&mut self) {
        self.end_turn = None;
    }

    #[allow(dead_code, reason = "read by the loop's end-turn handling")]
    pub(crate) fn take_end_turn(&mut self) -> Option<EndTurn> {
        self.end_turn.take()
    }
}

#[cfg(test)]
mod tests;
