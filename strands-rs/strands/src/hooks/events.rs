//! The hook events (TS `hooks/events.ts`).
//!
//! Sequential events borrow the agent's [`LocalAgent`] mutably, so their hooks can change history,
//! the system prompt, the model or the tools. Per-tool events (`BeforeToolCallEvent`,
//! `AfterToolCallEvent`, `ToolStreamUpdateEvent`, `ToolResultEvent`) are `'static` and hold the
//! batch's read-only [`ToolAgent`], because tools run concurrently.
//!
//! Fields are private: every field has a getter, and only the fields TS lets hooks write have
//! setters. `Serialize` writes the TS `toJSON()` shape, which leaves out the agent, the model,
//! the tool, the invocation state and the control fields.

use std::fmt;
use std::sync::Arc;

use serde::ser::{SerializeMap, Serializer};
use serde::{Deserialize, Serialize};

use crate::agent::InvokeArgs;
use crate::agent::invocation_context::InvocationContext;
use crate::agent::invocation_state::InvocationState;
use crate::agent::local_agent::LocalAgent;
use crate::agent::tool_agent::ToolAgent;
use crate::error::{DataError, Error, Result};
use crate::interrupt::{Interrupt, InterruptParams, InterruptSource};
use crate::models::model::{Model, Redaction};
use crate::models::streaming::ModelStreamEvent;
use crate::tools::tool::{Tool, ToolStreamEvent};
use crate::types::agent::AgentResult;
use crate::types::json::JsonValue;
use crate::types::messages::{ContentBlock, Message, StopReason, ToolResultBlock};
use crate::types::tools::ToolUse;

/// The error a tool call failed with, as [`AfterToolCallEvent::error`] holds it.
pub type ToolCallError = Arc<dyn std::error::Error + Send + Sync>;

/// A hook's request to cancel (TS `cancel: boolean | string`).
///
/// Without a message the agent uses the TS default text of the event that was cancelled.
///
/// ```
/// use strands::hooks::Cancel;
///
/// assert_eq!(Cancel::new().message(), None);
/// assert_eq!(Cancel::new().with_message("not allowed").message(), Some("not allowed"));
/// assert_eq!(Cancel::from("not allowed"), Cancel::new().with_message("not allowed"));
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cancel {
    message: Option<String>,
}

impl Cancel {
    /// A cancel with the default message (TS `true`).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the message (TS a string): the assistant response for a cancelled invocation or
    /// model call, or the error result of a cancelled tool.
    #[must_use]
    pub fn with_message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// The message, when set.
    #[must_use]
    pub fn message(&self) -> Option<&str> {
        self.message.as_deref()
    }
}

impl From<&str> for Cancel {
    fn from(message: &str) -> Self {
        Self::new().with_message(message)
    }
}

impl From<String> for Cancel {
    fn from(message: String) -> Self {
        Self::new().with_message(message)
    }
}

/// A hook's request to end the turn after the tool batch (TS `AfterToolsEvent.endTurn`).
///
/// The loop stops without calling the model again and appends a final assistant message; the
/// result's stop reason is [`StopReason::EndTurn`] in every case.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum EndTurn {
    /// The TS default text, `Turn ended early by hook after tool execution` (TS `true`).
    DefaultMessage,
    /// This text, used as the literal assistant content (TS a string).
    Message(String),
    /// These blocks, used as the assistant message content (TS `ContentBlock[]`).
    Content(Vec<ContentBlock>),
}

/// How a successful model call ended (TS `ModelStopData`, exported as `ModelStopResponse`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStopData {
    /// The assistant message the model produced.
    pub message: Message,
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// The redaction a guardrail applied, when one did.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub redaction: Option<Redaction>,
}

impl ModelStopData {
    /// Creates stop data without a redaction.
    #[must_use]
    pub fn new(message: Message, stop_reason: StopReason) -> Self {
        Self {
            message,
            stop_reason,
            redaction: None,
        }
    }

    /// Sets the redaction.
    #[must_use]
    pub fn with_redaction(mut self, redaction: Redaction) -> Self {
        self.redaction = Some(redaction);
        self
    }
}

/// Answers an event's `interrupt` from the agent's interrupt state (TS `interruptFromAgent`):
/// the stored response on resume, otherwise a new interrupt with source `hook`.
pub(crate) trait HookInterrupter: Send + Sync + 'static {
    #[expect(clippy::result_large_err, reason = "mirrors the events' interrupt")]
    fn interrupt(&self, id: String, params: InterruptParams) -> Result<JsonValue, Interrupt>;
}

/// The interrupts an interruptible event's hooks raised.
#[derive(Default)]
struct RaisedInterrupts {
    interrupter: Option<Arc<dyn HookInterrupter>>,
    raised: Vec<Interrupt>,
}

impl RaisedInterrupts {
    #[expect(clippy::result_large_err, reason = "mirrors the events' interrupt")]
    fn raise(&mut self, id: String, params: InterruptParams) -> Result<JsonValue, Interrupt> {
        let outcome = match &self.interrupter {
            Some(interrupter) => interrupter.interrupt(id, params),
            None => fresh_interrupt(id, params),
        };
        if let Err(interrupt) = &outcome {
            self.raised.push(interrupt.clone());
        }
        outcome
    }

    fn take(&mut self) -> Result<Vec<Interrupt>> {
        let raised = std::mem::take(&mut self.raised);
        check_duplicate_names(&raised)?;
        Ok(raised)
    }
}

#[expect(clippy::result_large_err, reason = "mirrors the events' interrupt")]
fn fresh_interrupt(id: String, params: InterruptParams) -> Result<JsonValue, Interrupt> {
    if let Some(response) = params.response {
        return Ok(response);
    }
    let mut interrupt = Interrupt::new(id, params.name).with_source(InterruptSource::Hook);
    interrupt.reason = params.reason;
    Err(interrupt)
}

/// Fails with the TS text when two hooks raised interrupts with the same name.
fn check_duplicate_names(interrupts: &[Interrupt]) -> Result<()> {
    let mut duplicates: Vec<&str> = Vec::new();
    for (index, interrupt) in interrupts.iter().enumerate() {
        let name = interrupt.name.as_str();
        let seen_before = interrupts[..index].iter().any(|earlier| earlier.name == name);
        if seen_before && !duplicates.contains(&name) {
            duplicates.push(name);
        }
    }
    if duplicates.is_empty() {
        return Ok(());
    }
    Err(Error::Data(DataError::new(format!(
        "interrupt_names=<{}> | duplicate interrupt names",
        duplicates.join(", ")
    ))))
}

fn type_only<S: Serializer>(serializer: S, event_type: &str) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(1))?;
    map.serialize_entry("type", event_type)?;
    map.end()
}

fn type_and<S: Serializer>(
    serializer: S,
    event_type: &str,
    key: &str,
    value: &impl Serialize,
) -> Result<S::Ok, S::Error> {
    let mut map = serializer.serialize_map(Some(2))?;
    map.serialize_entry("type", event_type)?;
    map.serialize_entry(key, value)?;
    map.end()
}

#[derive(Serialize)]
struct ErrorJson {
    message: String,
}

/// Fired once when the agent finishes initializing (TS `InitializedEvent`).
#[non_exhaustive]
pub struct InitializedEvent<'a> {
    agent: &'a mut LocalAgent,
}

impl<'a> InitializedEvent<'a> {
    pub(crate) fn new(agent: &'a mut LocalAgent) -> Self {
        Self { agent }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }
}

impl Serialize for InitializedEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_only(serializer, "initializedEvent")
    }
}

/// Fired at the start of an invocation, before the input is added (TS `BeforeInvocationEvent`).
///
/// Hooks may rewrite the input messages or cancel the invocation.
#[non_exhaustive]
pub struct BeforeInvocationEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    messages: Vec<Message>,
    cancel: Option<Cancel>,
}

impl<'a> BeforeInvocationEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        messages: Vec<Message>,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            messages,
            cancel: None,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// The input messages of this invocation.
    #[must_use]
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// The input messages, to change in place.
    pub fn messages_mut(&mut self) -> &mut Vec<Message> {
        &mut self.messages
    }

    /// Replaces the input messages.
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// The cancel request, when a hook set one.
    #[must_use]
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels the invocation, or withdraws a cancel with `None`. A message becomes the
    /// assistant response.
    pub fn set_cancel(&mut self, cancel: Option<Cancel>) {
        self.cancel = cancel;
    }

    pub(crate) fn into_parts(self) -> (Vec<Message>, Option<Cancel>) {
        (self.messages, self.cancel)
    }
}

impl Serialize for BeforeInvocationEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_only(serializer, "beforeInvocationEvent")
    }
}

/// Fired at the end of an invocation, also when it failed (TS `AfterInvocationEvent`). Same-order
/// hooks run in reverse registration order.
#[non_exhaustive]
pub struct AfterInvocationEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    resume: Option<InvokeArgs>,
}

impl<'a> AfterInvocationEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            resume: None,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// The input the agent runs again with after this invocation, if a hook set one.
    #[must_use]
    pub fn resume(&self) -> Option<&InvokeArgs> {
        self.resume.as_ref()
    }

    /// Runs the agent again with `resume` as input once this invocation ends (TS `resume`).
    /// Ignored when the invocation failed or its request reached a limit.
    pub fn set_resume(&mut self, resume: Option<InvokeArgs>) {
        self.resume = resume;
    }

    pub(crate) fn into_resume(self) -> Option<InvokeArgs> {
        self.resume
    }
}

impl Serialize for AfterInvocationEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_only(serializer, "afterInvocationEvent")
    }
}

/// Fired after a message is appended to the conversation (TS `MessageAddedEvent`).
#[non_exhaustive]
pub struct MessageAddedEvent<'a> {
    agent: &'a mut LocalAgent,
    message: Message,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
}

impl<'a> MessageAddedEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        message: Message,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            message,
            invocation_state,
            invocation_context,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The message that was added.
    #[must_use]
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }
}

impl Serialize for MessageAddedEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "messageAddedEvent", "message", &self.message)
    }
}

/// Fired before each model call (TS `BeforeModelCallEvent`). Hooks may cancel the call; request
/// rewriting belongs to model-call middleware.
#[non_exhaustive]
pub struct BeforeModelCallEvent<'a> {
    agent: &'a mut LocalAgent,
    model: Arc<dyn Model>,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    projected_input_tokens: Option<u64>,
    cancel: Option<Cancel>,
}

impl<'a> BeforeModelCallEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        model: Arc<dyn Model>,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            model,
            invocation_state,
            invocation_context,
            projected_input_tokens: None,
            cancel: None,
        }
    }

    pub(crate) fn with_projected_input_tokens(mut self, tokens: u64) -> Self {
        self.projected_input_tokens = Some(tokens);
        self
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The model about to be called.
    #[must_use]
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// The projected input tokens of the request, when a context manager estimated them.
    #[must_use]
    pub fn projected_input_tokens(&self) -> Option<u64> {
        self.projected_input_tokens
    }

    /// The cancel request, when a hook set one.
    #[must_use]
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels the model call, or withdraws a cancel with `None`. A message becomes the assistant
    /// response.
    pub fn set_cancel(&mut self, cancel: Option<Cancel>) {
        self.cancel = cancel;
    }
}

impl Serialize for BeforeModelCallEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", "beforeModelCallEvent")?;
        if let Some(tokens) = self.projected_input_tokens {
            map.serialize_entry("projectedInputTokens", &tokens)?;
        }
        map.end()
    }
}

/// Fired after each model call, successful or not (TS `AfterModelCallEvent`). Same-order hooks
/// run in reverse registration order.
///
/// A hook sets [`retry`](Self::set_retry) to call the model again; the retry strategy does this
/// for throttled calls.
#[non_exhaustive]
pub struct AfterModelCallEvent<'a> {
    agent: &'a mut LocalAgent,
    model: Arc<dyn Model>,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
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
        invocation_context: Option<InvocationContext>,
        attempt_count: u32,
    ) -> Self {
        Self {
            agent,
            model,
            invocation_state,
            invocation_context,
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

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The model that was called.
    #[must_use]
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// Which attempt of this model call this was, starting at 1.
    #[must_use]
    pub fn attempt_count(&self) -> u32 {
        self.attempt_count
    }

    /// How the call ended, when it succeeded.
    #[must_use]
    pub fn stop_data(&self) -> Option<&ModelStopData> {
        self.stop_data.as_ref()
    }

    /// The error the call failed with, when it failed.
    #[must_use]
    pub fn error(&self) -> Option<&'a Error> {
        self.error
    }

    /// Whether a hook asked to call the model again.
    #[must_use]
    pub fn retry(&self) -> bool {
        self.retry
    }

    /// Asks to discard this call and call the model again, or withdraws the request.
    pub fn set_retry(&mut self, retry: bool) {
        self.retry = retry;
    }
}

impl Serialize for AfterModelCallEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", "afterModelCallEvent")?;
        map.serialize_entry("attemptCount", &self.attempt_count)?;
        if let Some(stop_data) = &self.stop_data {
            map.serialize_entry("stopData", stop_data)?;
        }
        if let Some(error) = self.error {
            map.serialize_entry(
                "error",
                &ErrorJson {
                    message: error.to_string(),
                },
            )?;
        }
        map.end()
    }
}

/// Fired for each event the model streams (TS `ModelStreamUpdateEvent`).
#[non_exhaustive]
pub struct ModelStreamUpdateEvent<'a> {
    agent: &'a mut LocalAgent,
    event: ModelStreamEvent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
}

impl<'a> ModelStreamUpdateEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        event: ModelStreamEvent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            event,
            invocation_state,
            invocation_context,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The streamed model event.
    #[must_use]
    pub fn event(&self) -> &ModelStreamEvent {
        &self.event
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }
}

impl Serialize for ModelStreamUpdateEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "modelStreamUpdateEvent", "event", &self.event)
    }
}

/// Fired when the model completes a content block (TS `ContentBlockEvent`).
#[non_exhaustive]
pub struct ContentBlockEvent<'a> {
    agent: &'a mut LocalAgent,
    content_block: ContentBlock,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
}

impl<'a> ContentBlockEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        content_block: ContentBlock,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            content_block,
            invocation_state,
            invocation_context,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The completed block.
    #[must_use]
    pub fn content_block(&self) -> &ContentBlock {
        &self.content_block
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }
}

impl Serialize for ContentBlockEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(
            serializer,
            "contentBlockEvent",
            "contentBlock",
            &self.content_block,
        )
    }
}

/// Fired when the model completes its message (TS `ModelMessageEvent`).
#[non_exhaustive]
pub struct ModelMessageEvent<'a> {
    agent: &'a mut LocalAgent,
    message: Message,
    stop_reason: StopReason,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
}

impl<'a> ModelMessageEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        message: Message,
        stop_reason: StopReason,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            message,
            stop_reason,
            invocation_state,
            invocation_context,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The assistant message.
    #[must_use]
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// Why the model stopped.
    #[must_use]
    pub fn stop_reason(&self) -> &StopReason {
        &self.stop_reason
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }
}

impl Serialize for ModelMessageEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(3))?;
        map.serialize_entry("type", "modelMessageEvent")?;
        map.serialize_entry("message", &self.message)?;
        map.serialize_entry("stopReason", &self.stop_reason)?;
        map.end()
    }
}

/// Fired with the invocation's result, before it is returned (TS `AgentResultEvent`).
#[non_exhaustive]
pub struct AgentResultEvent<'a> {
    agent: &'a mut LocalAgent,
    result: AgentResult,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
}

impl<'a> AgentResultEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        result: AgentResult,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            result,
            invocation_state,
            invocation_context,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The invocation's result.
    #[must_use]
    pub fn result(&self) -> &AgentResult {
        &self.result
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }
}

impl Serialize for AgentResultEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "agentResultEvent", "result", &self.result)
    }
}

/// Fired when the invocation stops on interrupts, once per interrupt (TS `InterruptEvent`).
#[non_exhaustive]
pub struct InterruptEvent<'a> {
    agent: &'a mut LocalAgent,
    interrupt: Interrupt,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
}

impl<'a> InterruptEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        interrupt: Interrupt,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            interrupt,
            invocation_state,
            invocation_context,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The interrupt.
    #[must_use]
    pub fn interrupt(&self) -> &Interrupt {
        &self.interrupt
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }
}

impl Serialize for InterruptEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "interruptEvent", "interrupt", &self.interrupt)
    }
}

/// Fired before a tool batch runs, with the assistant message holding the tool uses (TS
/// `BeforeToolsEvent`). Hooks may cancel the batch or [`interrupt`](Self::interrupt) it.
#[non_exhaustive]
pub struct BeforeToolsEvent<'a> {
    agent: &'a mut LocalAgent,
    message: Message,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    cancel: Option<Cancel>,
    interrupts: RaisedInterrupts,
}

impl<'a> BeforeToolsEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        message: Message,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            message,
            invocation_state,
            invocation_context,
            cancel: None,
            interrupts: RaisedInterrupts::default(),
        }
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "fired by the agent"))]
    pub(crate) fn with_interrupter(mut self, interrupter: Arc<dyn HookInterrupter>) -> Self {
        self.interrupts.interrupter = Some(interrupter);
        self
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The assistant message with the tool uses.
    #[must_use]
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// The cancel request, when a hook set one.
    #[must_use]
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels every tool of the batch, or withdraws a cancel with `None`. A message becomes the
    /// tools' error result.
    pub fn set_cancel(&mut self, cancel: Option<Cancel>) {
        self.cancel = cancel;
    }

    /// Interrupts the batch for human input (TS `interrupt`), with id `hook:beforeTools:<name>`.
    ///
    /// On resume this returns the stored response. Otherwise it returns the new interrupt, which
    /// the agent collects when the hooks finish; return from the hook then.
    ///
    /// # Errors
    ///
    /// The interrupt when no response is available yet.
    #[expect(clippy::result_large_err, reason = "the same shape as ToolContext::interrupt")]
    pub fn interrupt(&mut self, params: impl Into<InterruptParams>) -> Result<JsonValue, Interrupt> {
        let params = params.into();
        let id = format!("hook:beforeTools:{}", params.name);
        self.interrupts.raise(id, params)
    }

    /// The interrupts the hooks raised, failing when two share a name (TS text
    /// `interrupt_names=<..> | duplicate interrupt names`).
    pub(crate) fn take_interrupts(&mut self) -> Result<Vec<Interrupt>> {
        self.interrupts.take()
    }
}

impl Serialize for BeforeToolsEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "beforeToolsEvent", "message", &self.message)
    }
}

/// Fired after a tool batch, with the user message holding the results (TS `AfterToolsEvent`).
/// Same-order hooks run in reverse registration order.
#[non_exhaustive]
pub struct AfterToolsEvent<'a> {
    agent: &'a mut LocalAgent,
    message: Message,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    end_turn: Option<EndTurn>,
}

impl<'a> AfterToolsEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        message: Message,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
    ) -> Self {
        Self {
            agent,
            message,
            invocation_state,
            invocation_context,
            end_turn: None,
        }
    }

    /// The agent.
    #[must_use]
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, to change.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }

    /// The user message with the tool results.
    #[must_use]
    pub fn message(&self) -> &Message {
        &self.message
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.invocation_context.as_ref()
    }

    /// The end-turn request, when a hook set one.
    #[must_use]
    pub fn end_turn(&self) -> Option<&EndTurn> {
        self.end_turn.as_ref()
    }

    /// Ends the turn after this batch without calling the model again, or withdraws the request
    /// with `None`.
    pub fn set_end_turn(&mut self, end_turn: Option<EndTurn>) {
        self.end_turn = end_turn;
    }
}

impl Serialize for AfterToolsEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "afterToolsEvent", "message", &self.message)
    }
}

/// Fired before a tool runs (TS `BeforeToolCallEvent`).
///
/// Hooks may rewrite the tool use, pick another tool to run, cancel the call or
/// [`interrupt`](Self::interrupt) it for approval.
#[non_exhaustive]
pub struct BeforeToolCallEvent {
    agent: ToolAgent,
    tool_use: ToolUse,
    tool: Option<Arc<dyn Tool>>,
    invocation_state: InvocationState,
    cancel: Option<Cancel>,
    selected_tool: Option<Arc<dyn Tool>>,
    interrupts: RaisedInterrupts,
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
            interrupts: RaisedInterrupts::default(),
        }
    }

    #[cfg_attr(not(test), expect(dead_code, reason = "fired by the tool executors"))]
    pub(crate) fn with_interrupter(mut self, interrupter: Arc<dyn HookInterrupter>) -> Self {
        self.interrupts.interrupter = Some(interrupter);
        self
    }

    /// The agent as the tool batch sees it.
    #[must_use]
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The tool use about to run.
    #[must_use]
    pub fn tool_use(&self) -> &ToolUse {
        &self.tool_use
    }

    /// The tool use, to change in place.
    pub fn tool_use_mut(&mut self) -> &mut ToolUse {
        &mut self.tool_use
    }

    /// Replaces the tool use.
    pub fn set_tool_use(&mut self, tool_use: ToolUse) {
        self.tool_use = tool_use;
    }

    /// The registry tool the model asked for, `None` when no tool has that name.
    #[must_use]
    pub fn tool(&self) -> Option<&Arc<dyn Tool>> {
        self.tool.as_ref()
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.agent.invocation_context()
    }

    /// The cancel request, when a hook set one.
    #[must_use]
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels the call, or withdraws a cancel with `None`. A message becomes the tool's error
    /// result.
    pub fn set_cancel(&mut self, cancel: Option<Cancel>) {
        self.cancel = cancel;
    }

    /// The tool a hook chose to run instead of [`tool`](Self::tool).
    #[must_use]
    pub fn selected_tool(&self) -> Option<&Arc<dyn Tool>> {
        self.selected_tool.as_ref()
    }

    /// Runs this tool instead of the registry tool, or clears the choice with `None`.
    pub fn set_selected_tool(&mut self, tool: Option<Arc<dyn Tool>>) {
        self.selected_tool = tool;
    }

    /// Interrupts the call for human input (TS `interrupt`), with id
    /// `hook:beforeToolCall:<toolUseId>:<name>`.
    ///
    /// On resume this returns the stored response. Otherwise it returns the new interrupt, which
    /// the agent collects when the hooks finish; return from the hook then.
    ///
    /// ```
    /// use strands::hooks::{BeforeToolCallEvent, HookRegistry};
    ///
    /// let registry = HookRegistry::new();
    /// registry.add_hook(|event: &mut BeforeToolCallEvent| {
    ///     if event.tool_use().name != "delete_file" {
    ///         return;
    ///     }
    ///     let Ok(answer) = event.interrupt("approve") else {
    ///         return;
    ///     };
    ///     if answer != "yes" {
    ///         event.set_cancel(Some("not approved".into()));
    ///     }
    /// });
    /// ```
    ///
    /// # Errors
    ///
    /// The interrupt when no response is available yet.
    #[expect(clippy::result_large_err, reason = "the same shape as ToolContext::interrupt")]
    pub fn interrupt(&mut self, params: impl Into<InterruptParams>) -> Result<JsonValue, Interrupt> {
        let params = params.into();
        let id = format!(
            "hook:beforeToolCall:{}:{}",
            self.tool_use.tool_use_id, params.name
        );
        self.interrupts.raise(id, params)
    }

    /// The interrupts the hooks raised, failing when two share a name (TS text
    /// `interrupt_names=<..> | duplicate interrupt names`).
    pub(crate) fn take_interrupts(&mut self) -> Result<Vec<Interrupt>> {
        self.interrupts.take()
    }
}

impl fmt::Debug for BeforeToolCallEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BeforeToolCallEvent")
            .field("tool_use", &self.tool_use)
            .field("cancel", &self.cancel)
            .finish_non_exhaustive()
    }
}

impl Serialize for BeforeToolCallEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "beforeToolCallEvent", "toolUse", &self.tool_use)
    }
}

/// Fired after a tool ran, also when it failed (TS `AfterToolCallEvent`). Same-order hooks run in
/// reverse registration order.
///
/// Hooks may replace the result or set [`retry`](Self::set_retry) to run the tool again.
#[non_exhaustive]
pub struct AfterToolCallEvent {
    agent: ToolAgent,
    tool_use: ToolUse,
    tool: Option<Arc<dyn Tool>>,
    result: ToolResultBlock,
    error: Option<ToolCallError>,
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

    pub(crate) fn with_error(mut self, error: ToolCallError) -> Self {
        self.error = Some(error);
        self
    }

    /// The agent as the tool batch sees it.
    #[must_use]
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The tool use that ran.
    #[must_use]
    pub fn tool_use(&self) -> &ToolUse {
        &self.tool_use
    }

    /// The tool that ran, `None` when no tool had that name.
    #[must_use]
    pub fn tool(&self) -> Option<&Arc<dyn Tool>> {
        self.tool.as_ref()
    }

    /// The tool's result.
    #[must_use]
    pub fn result(&self) -> &ToolResultBlock {
        &self.result
    }

    /// Replaces the result the model sees.
    pub fn set_result(&mut self, result: ToolResultBlock) {
        self.result = result;
    }

    /// The error the call failed with, when it failed.
    #[must_use]
    pub fn error(&self) -> Option<&(dyn std::error::Error + Send + Sync + 'static)> {
        self.error.as_deref()
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.agent.invocation_context()
    }

    /// Whether a hook asked to run the tool again.
    #[must_use]
    pub fn retry(&self) -> bool {
        self.retry
    }

    /// Asks to discard this result and run the tool again, or withdraws the request.
    pub fn set_retry(&mut self, retry: bool) {
        self.retry = retry;
    }
}

impl fmt::Debug for AfterToolCallEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AfterToolCallEvent")
            .field("tool_use", &self.tool_use)
            .field("result", &self.result)
            .field("retry", &self.retry)
            .finish_non_exhaustive()
    }
}

impl Serialize for AfterToolCallEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("type", "afterToolCallEvent")?;
        map.serialize_entry("toolUse", &self.tool_use)?;
        map.serialize_entry("result", &self.result)?;
        if let Some(error) = &self.error {
            map.serialize_entry(
                "error",
                &ErrorJson {
                    message: error.to_string(),
                },
            )?;
        }
        map.end()
    }
}

/// Fired for each progress event a tool streams (TS `ToolStreamUpdateEvent`).
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

    /// The agent as the tool batch sees it.
    #[must_use]
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The tool's progress event.
    #[must_use]
    pub fn event(&self) -> &ToolStreamEvent {
        &self.event
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.agent.invocation_context()
    }
}

impl fmt::Debug for ToolStreamUpdateEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolStreamUpdateEvent")
            .field("event", &self.event)
            .finish_non_exhaustive()
    }
}

impl Serialize for ToolStreamUpdateEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "toolStreamUpdateEvent", "event", &self.event)
    }
}

/// Fired with each final tool result (TS `ToolResultEvent`).
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

    /// The agent as the tool batch sees it.
    #[must_use]
    pub fn agent(&self) -> &ToolAgent {
        &self.agent
    }

    /// The tool result.
    #[must_use]
    pub fn result(&self) -> &ToolResultBlock {
        &self.result
    }

    /// The caller's invocation state.
    #[must_use]
    pub fn invocation_state(&self) -> &InvocationState {
        &self.invocation_state
    }

    /// The request this invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.agent.invocation_context()
    }
}

impl fmt::Debug for ToolResultEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolResultEvent")
            .field("result", &self.result)
            .finish_non_exhaustive()
    }
}

impl Serialize for ToolResultEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        type_and(serializer, "toolResultEvent", "result", &self.result)
    }
}

#[cfg(test)]
mod tests;
