//! Hook events (TS `hooks/events.ts`).
//!
//! Sequential events borrow the agent's [`LocalAgent`] mutably. Per-tool events
//! ([`BeforeToolCallEvent`], [`AfterToolCallEvent`], [`ToolStreamUpdateEvent`],
//! [`ToolResultEvent`]) are `'static` and carry a [`ToolAgent`]. Fields are read through
//! getters; only the fields TS lets hooks write have setters.
//!
//! Each event keeps its serializable part in a `<Event>Data` record, which is what the agent
//! stream yields once the hooks have run.

use std::fmt;
use std::sync::{Arc, PoisonError};

use serde::{Serialize, Serializer};

use crate::agent::{AgentResult, InvocationContext, InvocationState, InvokeArgs, LocalAgent, ToolAgent};
use crate::error::Error;
use crate::interrupt::{Interrupt, InterruptSource};
use crate::models::{Model, ModelStreamEvent, Redaction};
use crate::tools::{Tool, ToolStreamEvent, ToolUse};
use crate::types::{ContentBlock, InterruptParams, JsonValue, Message, StopReason, ToolResultBlock};

/// A hook's request to cancel a step (TS `cancel: true | string`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum Cancel {
    /// Cancel with the SDK's default message.
    Default,
    /// Cancel with this message. An empty message cancels nothing, as in TS.
    Message(String),
}

impl Cancel {
    /// The message to use, or `None` when this doesn't cancel.
    pub(crate) fn resolve(&self, default: &str) -> Option<String> {
        match self {
            Self::Default => Some(default.to_owned()),
            Self::Message(message) if message.is_empty() => None,
            Self::Message(message) => Some(message.clone()),
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
    /// End with the SDK's default message.
    Default,
    /// End with this text as the final assistant message. Empty text ends nothing.
    Text(String),
    /// End with these blocks as the final assistant message. No blocks ends nothing.
    Content(Vec<ContentBlock>),
}

impl EndTurn {
    pub(crate) fn resolve(&self, default: &str) -> Option<Vec<ContentBlock>> {
        match self {
            Self::Default => Some(vec![ContentBlock::from(default)]),
            Self::Text(text) if text.is_empty() => None,
            Self::Text(text) => Some(vec![ContentBlock::from(text.as_str())]),
            Self::Content(content) if content.is_empty() => None,
            Self::Content(content) => Some(content.clone()),
        }
    }
}

impl From<&str> for EndTurn {
    fn from(text: &str) -> Self {
        Self::Text(text.to_owned())
    }
}

impl From<String> for EndTurn {
    fn from(text: String) -> Self {
        Self::Text(text)
    }
}

impl From<Vec<ContentBlock>> for EndTurn {
    fn from(content: Vec<ContentBlock>) -> Self {
        Self::Content(content)
    }
}

/// The message of an error attached to an event record (TS `error: { message }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[non_exhaustive]
pub struct EventErrorData {
    /// The error message.
    pub message: String,
}

impl EventErrorData {
    fn of(error: &(impl fmt::Display + ?Sized)) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

fn wrap_tool_result<S: Serializer>(result: &ToolResultBlock, serializer: S) -> Result<S::Ok, S::Error> {
    #[derive(Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Wrapped<'a> {
        tool_result: &'a ToolResultBlock,
    }
    Wrapped { tool_result: result }.serialize(serializer)
}

macro_rules! sequential_accessors {
    ($event:ident) => {
        impl<'a> $event<'a> {
            /// The agent.
            pub fn agent(&self) -> &LocalAgent {
                self.agent
            }

            /// The agent, mutably.
            pub fn agent_mut(&mut self) -> &mut LocalAgent {
                self.agent
            }

            /// The caller's invocation state.
            pub fn invocation_state(&self) -> &InvocationState {
                &self.invocation_state
            }

            /// The invocation context.
            pub fn invocation_context(&self) -> Option<&InvocationContext> {
                self.invocation_context.as_ref()
            }

            #[allow(
                dead_code,
                reason = "generated for every event; the loop uses most of them"
            )]
            pub(crate) fn into_data(self) -> <Self as EventData>::Data {
                self.data
            }

            #[allow(
                dead_code,
                reason = "generated for every event; the hook recorder reads most of them"
            )]
            pub(crate) fn data(&self) -> &<Self as EventData>::Data {
                &self.data
            }
        }

        impl fmt::Debug for $event<'_> {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($event))
                    .field("data", &self.data)
                    .finish_non_exhaustive()
            }
        }
    };
}

macro_rules! tool_accessors {
    ($event:ident) => {
        impl $event {
            /// The agent view of the tool batch.
            pub fn agent(&self) -> &ToolAgent {
                &self.agent
            }

            /// The caller's invocation state.
            pub fn invocation_state(&self) -> &InvocationState {
                &self.invocation_state
            }

            /// The invocation context.
            pub fn invocation_context(&self) -> Option<&InvocationContext> {
                self.agent.invocation_context()
            }

            #[allow(
                dead_code,
                reason = "generated for every event; the loop uses most of them"
            )]
            pub(crate) fn into_data(self) -> <Self as EventData>::Data {
                self.data
            }

            #[allow(
                dead_code,
                reason = "generated for every event; the hook recorder reads most of them"
            )]
            pub(crate) fn data(&self) -> &<Self as EventData>::Data {
                &self.data
            }
        }

        impl fmt::Debug for $event {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($event))
                    .field("data", &self.data)
                    .finish_non_exhaustive()
            }
        }
    };
}

/// Links an event to its serializable record.
pub(crate) trait EventData {
    type Data;
}

macro_rules! record {
    ($(#[$meta:meta])* $data:ident, $tag:literal { $($(#[$fmeta:meta])* $field:ident: $ty:ty),* $(,)? }) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize)]
        #[serde(tag = "type", rename = $tag, rename_all = "camelCase")]
        #[non_exhaustive]
        pub struct $data {
            $($(#[$fmeta])* pub $field: $ty,)*
        }
    };
}

// ---------------------------------------------------------------------------------------------
// InitializedEvent

/// Fired once when the agent finishes initializing (TS `InitializedEvent`).
pub struct InitializedEvent<'a> {
    agent: &'a mut LocalAgent,
}

impl<'a> InitializedEvent<'a> {
    pub(crate) fn new(agent: &'a mut LocalAgent) -> Self {
        Self { agent }
    }

    /// The agent.
    pub fn agent(&self) -> &LocalAgent {
        self.agent
    }

    /// The agent, mutably.
    pub fn agent_mut(&mut self) -> &mut LocalAgent {
        self.agent
    }
}

impl fmt::Debug for InitializedEvent<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("InitializedEvent").finish_non_exhaustive()
    }
}

// ---------------------------------------------------------------------------------------------
// BeforeInvocationEvent

record!(
    /// The record of a [`BeforeInvocationEvent`].
    BeforeInvocationEventData, "beforeInvocationEvent" {}
);

/// Fired at the start of an invocation (TS `BeforeInvocationEvent`).
///
/// Hooks may rewrite the input messages or cancel the invocation.
pub struct BeforeInvocationEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: BeforeInvocationEventData,
    messages: Vec<Message>,
    cancel: Option<Cancel>,
}

impl EventData for BeforeInvocationEvent<'_> {
    type Data = BeforeInvocationEventData;
}
sequential_accessors!(BeforeInvocationEvent);

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
            data: BeforeInvocationEventData {},
            messages,
            cancel: None,
        }
    }

    /// The input messages about to be appended.
    pub fn messages(&self) -> &[Message] {
        &self.messages
    }

    /// Replaces the input messages.
    pub fn set_messages(&mut self, messages: Vec<Message>) {
        self.messages = messages;
    }

    /// The cancel request, if any.
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels the invocation; the agent answers with the message instead.
    pub fn set_cancel(&mut self, cancel: impl Into<Cancel>) {
        self.cancel = Some(cancel.into());
    }

    /// Withdraws a cancel request.
    pub fn clear_cancel(&mut self) {
        self.cancel = None;
    }

    pub(crate) fn into_parts(self) -> (Vec<Message>, Option<Cancel>) {
        (self.messages, self.cancel)
    }
}

// ---------------------------------------------------------------------------------------------
// AfterInvocationEvent

record!(
    /// The record of an [`AfterInvocationEvent`].
    AfterInvocationEventData, "afterInvocationEvent" {}
);

/// Fired at the end of an invocation, also after a failure (TS `AfterInvocationEvent`).
///
/// Setting `resume` runs another invocation with that input before the result is returned;
/// it is ignored after an error or when a limit was reached.
pub struct AfterInvocationEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: AfterInvocationEventData,
    resume: Option<InvokeArgs>,
}

impl EventData for AfterInvocationEvent<'_> {
    type Data = AfterInvocationEventData;
}
sequential_accessors!(AfterInvocationEvent);

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
            data: AfterInvocationEventData {},
            resume: None,
        }
    }

    /// The input to resume with, if set.
    pub fn resume(&self) -> Option<&InvokeArgs> {
        self.resume.as_ref()
    }

    /// Resumes the agent with `args` after this invocation.
    pub fn set_resume(&mut self, args: impl Into<InvokeArgs>) {
        self.resume = Some(args.into());
    }

    /// Withdraws a resume request.
    pub fn clear_resume(&mut self) {
        self.resume = None;
    }

    pub(crate) fn take_resume(&mut self) -> Option<InvokeArgs> {
        self.resume.take()
    }
}

// ---------------------------------------------------------------------------------------------
// MessageAddedEvent

record!(
    /// The record of a [`MessageAddedEvent`].
    MessageAddedEventData, "messageAddedEvent" {
        /// The appended message.
        message: Message,
    }
);

/// Fired after a message is appended to the history (TS `MessageAddedEvent`).
pub struct MessageAddedEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: MessageAddedEventData,
}

impl EventData for MessageAddedEvent<'_> {
    type Data = MessageAddedEventData;
}
sequential_accessors!(MessageAddedEvent);

impl<'a> MessageAddedEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        message: Message,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: MessageAddedEventData { message },
        }
    }

    /// The appended message.
    pub fn message(&self) -> &Message {
        &self.data.message
    }
}

// ---------------------------------------------------------------------------------------------
// BeforeToolCallEvent

/// The record of a [`BeforeToolCallEvent`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename = "beforeToolCallEvent", rename_all = "camelCase")]
#[non_exhaustive]
pub struct BeforeToolCallEventData {
    /// The tool call.
    pub tool_use: ToolUse,
    #[serde(skip)]
    pub(crate) denied: bool,
}

/// Fired before each tool call (TS `BeforeToolCallEvent`).
///
/// Hooks may rewrite the call, pick a different tool, cancel it, or interrupt for approval.
pub struct BeforeToolCallEvent {
    agent: ToolAgent,
    invocation_state: InvocationState,
    data: BeforeToolCallEventData,
    tool: Option<Arc<dyn Tool>>,
    cancel: Option<Cancel>,
    selected_tool: Option<Arc<dyn Tool>>,
    interrupts: Vec<Interrupt>,
}

impl EventData for BeforeToolCallEvent {
    type Data = BeforeToolCallEventData;
}
tool_accessors!(BeforeToolCallEvent);

impl BeforeToolCallEvent {
    pub(crate) fn new(
        agent: ToolAgent,
        invocation_state: InvocationState,
        tool_use: ToolUse,
        tool: Option<Arc<dyn Tool>>,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            data: BeforeToolCallEventData {
                tool_use,
                denied: false,
            },
            tool,
            cancel: None,
            selected_tool: None,
            interrupts: Vec::new(),
        }
    }

    /// The tool call.
    pub fn tool_use(&self) -> &ToolUse {
        &self.data.tool_use
    }

    /// Rewrites the tool call. The tool use id always stays the model's.
    pub fn set_tool_use(&mut self, tool_use: ToolUse) {
        self.data.tool_use = tool_use;
    }

    /// The registered tool for the call's name, if any.
    pub fn tool(&self) -> Option<&Arc<dyn Tool>> {
        self.tool.as_ref()
    }

    /// The tool a hook picked to run instead.
    pub fn selected_tool(&self) -> Option<&Arc<dyn Tool>> {
        self.selected_tool.as_ref()
    }

    /// Runs `tool` for this call instead of the registered one.
    pub fn set_selected_tool(&mut self, tool: Arc<dyn Tool>) {
        self.selected_tool = Some(tool);
    }

    /// The cancel request, if any.
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels the call; the model sees an error result with the message.
    pub fn set_cancel(&mut self, cancel: impl Into<Cancel>) {
        self.cancel = Some(cancel.into());
    }

    /// Withdraws a cancel request.
    pub fn clear_cancel(&mut self) {
        self.cancel = None;
    }

    /// Asks the caller for input before the call runs (TS `event.interrupt`).
    ///
    /// On resume this returns the caller's response. Otherwise it records the interrupt and
    /// returns `None`; the agent stops with [`StopReason::Interrupt`] once the hooks finish.
    pub fn interrupt(&mut self, params: impl Into<InterruptParams>) -> Option<JsonValue> {
        let params = params.into();
        let id = format!(
            "hook:beforeToolCall:{}:{}",
            self.data.tool_use.tool_use_id, params.name
        );
        let raised = self
            .agent
            .interrupt_state()
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .interrupt(&id, params, InterruptSource::Hook);
        match raised {
            Ok(response) => Some(response),
            Err(interrupt) => {
                self.interrupts.push(*interrupt);
                None
            }
        }
    }

    pub(crate) fn take_interrupts(&mut self) -> Vec<Interrupt> {
        std::mem::take(&mut self.interrupts)
    }

    pub(crate) fn into_parts(mut self) -> (BeforeToolCallEventData, Option<String>, Option<Arc<dyn Tool>>) {
        let cancel = self
            .cancel
            .and_then(|cancel| cancel.resolve("Tool cancelled by hook"));
        self.data.denied = cancel.is_some();
        (self.data, cancel, self.selected_tool)
    }
}

// ---------------------------------------------------------------------------------------------
// AfterToolCallEvent

record!(
    /// The record of an [`AfterToolCallEvent`].
    AfterToolCallEventData, "afterToolCallEvent" {
        /// The tool call.
        tool_use: ToolUse,
        /// The tool result.
        #[serde(serialize_with = "wrap_tool_result")]
        result: ToolResultBlock,
        /// The error the tool failed with.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<EventErrorData>,
    }
);

/// Fired after each tool call (TS `AfterToolCallEvent`).
///
/// Hooks may replace the result or ask for the call to run again.
pub struct AfterToolCallEvent {
    agent: ToolAgent,
    invocation_state: InvocationState,
    data: AfterToolCallEventData,
    tool: Option<Arc<dyn Tool>>,
    retry: bool,
}

impl EventData for AfterToolCallEvent {
    type Data = AfterToolCallEventData;
}
tool_accessors!(AfterToolCallEvent);

impl AfterToolCallEvent {
    pub(crate) fn new(
        agent: ToolAgent,
        invocation_state: InvocationState,
        tool_use: ToolUse,
        tool: Option<Arc<dyn Tool>>,
        result: ToolResultBlock,
    ) -> Self {
        let error = result.error.as_deref().map(EventErrorData::of);
        Self {
            agent,
            invocation_state,
            data: AfterToolCallEventData {
                tool_use,
                result,
                error,
            },
            tool,
            retry: false,
        }
    }

    /// The tool call.
    pub fn tool_use(&self) -> &ToolUse {
        &self.data.tool_use
    }

    /// The tool that ran, if any.
    pub fn tool(&self) -> Option<&Arc<dyn Tool>> {
        self.tool.as_ref()
    }

    /// The result.
    pub fn result(&self) -> &ToolResultBlock {
        &self.data.result
    }

    /// Replaces the result.
    pub fn set_result(&mut self, result: ToolResultBlock) {
        self.data.result = result;
    }

    /// The error the tool failed with.
    pub fn error(&self) -> Option<&EventErrorData> {
        self.data.error.as_ref()
    }

    /// Whether the call will run again.
    pub fn retry(&self) -> bool {
        self.retry
    }

    /// Runs the call again (ignored once the invocation is cancelled).
    pub fn set_retry(&mut self, retry: bool) {
        self.retry = retry;
    }
}

// ---------------------------------------------------------------------------------------------
// BeforeModelCallEvent

record!(
    /// The record of a [`BeforeModelCallEvent`].
    BeforeModelCallEventData, "beforeModelCallEvent" {
        /// The estimated input tokens of the request.
        #[serde(skip_serializing_if = "Option::is_none")]
        projected_input_tokens: Option<u64>,
    }
);

/// Fired before each model call (TS `BeforeModelCallEvent`).
///
/// Hooks may cancel the call. Rewriting the request is `InvokeModelStage` middleware.
pub struct BeforeModelCallEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: BeforeModelCallEventData,
    model: Arc<dyn Model>,
    cancel: Option<Cancel>,
}

impl EventData for BeforeModelCallEvent<'_> {
    type Data = BeforeModelCallEventData;
}
sequential_accessors!(BeforeModelCallEvent);

impl<'a> BeforeModelCallEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        model: Arc<dyn Model>,
        projected_input_tokens: Option<u64>,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: BeforeModelCallEventData {
                projected_input_tokens,
            },
            model,
            cancel: None,
        }
    }

    /// The model about to be called.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// The estimated input tokens of the request.
    pub fn projected_input_tokens(&self) -> Option<u64> {
        self.data.projected_input_tokens
    }

    /// The cancel request, if any.
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Skips the call; the message becomes the model's answer.
    pub fn set_cancel(&mut self, cancel: impl Into<Cancel>) {
        self.cancel = Some(cancel.into());
    }

    /// Withdraws a cancel request.
    pub fn clear_cancel(&mut self) {
        self.cancel = None;
    }

    pub(crate) fn take_cancel(&mut self) -> Option<Cancel> {
        self.cancel.take()
    }
}

// ---------------------------------------------------------------------------------------------
// AfterModelCallEvent

/// What a successful model call produced (TS `ModelStopData`).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct ModelStopData {
    /// The model's message.
    pub message: Message,
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// Set when a guardrail redacted the input.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction: Option<Redaction>,
}

impl ModelStopData {
    /// Stop data for `message` and `stop_reason`.
    pub fn new(message: Message, stop_reason: StopReason) -> Self {
        Self {
            message,
            stop_reason,
            redaction: None,
        }
    }
}

record!(
    /// The record of an [`AfterModelCallEvent`].
    AfterModelCallEventData, "afterModelCallEvent" {
        /// The attempt number, from 1.
        attempt_count: u32,
        /// What the call produced, when it succeeded.
        #[serde(skip_serializing_if = "Option::is_none")]
        stop_data: Option<ModelStopData>,
        /// The error the call failed with.
        #[serde(skip_serializing_if = "Option::is_none")]
        error: Option<EventErrorData>,
    }
);

/// Fired after each model call, successful or not (TS `AfterModelCallEvent`).
///
/// Hooks may ask for the call to run again; the retry strategies work this way.
pub struct AfterModelCallEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: AfterModelCallEventData,
    model: Arc<dyn Model>,
    error: Option<Error>,
    retry: bool,
}

impl EventData for AfterModelCallEvent<'_> {
    type Data = AfterModelCallEventData;
}
sequential_accessors!(AfterModelCallEvent);

impl<'a> AfterModelCallEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        model: Arc<dyn Model>,
        attempt_count: u32,
        outcome: Result<ModelStopData, Error>,
    ) -> Self {
        let (stop_data, error) = match outcome {
            Ok(stop_data) => (Some(stop_data), None),
            Err(error) => (None, Some(error)),
        };
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: AfterModelCallEventData {
                attempt_count,
                stop_data,
                error: error.as_ref().map(EventErrorData::of),
            },
            model,
            error,
            retry: false,
        }
    }

    /// The model that was called.
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.model
    }

    /// The attempt number, from 1.
    pub fn attempt_count(&self) -> u32 {
        self.data.attempt_count
    }

    /// What the call produced, when it succeeded.
    pub fn stop_data(&self) -> Option<&ModelStopData> {
        self.data.stop_data.as_ref()
    }

    /// The error the call failed with.
    pub fn error(&self) -> Option<&Error> {
        self.error.as_ref()
    }

    /// Whether the call will run again.
    pub fn retry(&self) -> bool {
        self.retry
    }

    /// Runs the call again.
    pub fn set_retry(&mut self, retry: bool) {
        self.retry = retry;
    }

    pub(crate) fn take_error(&mut self) -> Option<Error> {
        self.error.take()
    }
}

// ---------------------------------------------------------------------------------------------
// ModelStreamUpdateEvent

record!(
    /// The record of a [`ModelStreamUpdateEvent`].
    ModelStreamUpdateEventData, "modelStreamUpdateEvent" {
        /// The model stream event.
        event: ModelStreamEvent,
    }
);

impl ModelStreamUpdateEventData {
    /// The text of a text delta, if this is one.
    pub fn text_delta(&self) -> Option<&str> {
        self.event.text_delta()
    }
}

/// Fired for each raw model stream event (TS `ModelStreamUpdateEvent`).
pub struct ModelStreamUpdateEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: ModelStreamUpdateEventData,
}

impl EventData for ModelStreamUpdateEvent<'_> {
    type Data = ModelStreamUpdateEventData;
}
sequential_accessors!(ModelStreamUpdateEvent);

impl<'a> ModelStreamUpdateEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        event: ModelStreamEvent,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: ModelStreamUpdateEventData { event },
        }
    }

    /// The model stream event.
    pub fn event(&self) -> &ModelStreamEvent {
        &self.data.event
    }
}

// ---------------------------------------------------------------------------------------------
// ContentBlockEvent

record!(
    /// The record of a [`ContentBlockEvent`].
    ContentBlockEventData, "contentBlockEvent" {
        /// The completed block.
        content_block: ContentBlock,
    }
);

/// Fired when the model completes a content block (TS `ContentBlockEvent`).
pub struct ContentBlockEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: ContentBlockEventData,
}

impl EventData for ContentBlockEvent<'_> {
    type Data = ContentBlockEventData;
}
sequential_accessors!(ContentBlockEvent);

impl<'a> ContentBlockEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        content_block: ContentBlock,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: ContentBlockEventData { content_block },
        }
    }

    /// The completed block.
    pub fn content_block(&self) -> &ContentBlock {
        &self.data.content_block
    }
}

// ---------------------------------------------------------------------------------------------
// ModelMessageEvent

record!(
    /// The record of a [`ModelMessageEvent`].
    ModelMessageEventData, "modelMessageEvent" {
        /// The model's message.
        message: Message,
        /// Why the model stopped.
        stop_reason: StopReason,
    }
);

/// Fired when the model completes its message (TS `ModelMessageEvent`).
pub struct ModelMessageEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: ModelMessageEventData,
}

impl EventData for ModelMessageEvent<'_> {
    type Data = ModelMessageEventData;
}
sequential_accessors!(ModelMessageEvent);

impl<'a> ModelMessageEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        message: Message,
        stop_reason: StopReason,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: ModelMessageEventData { message, stop_reason },
        }
    }

    /// The model's message.
    pub fn message(&self) -> &Message {
        &self.data.message
    }

    /// Why the model stopped.
    pub fn stop_reason(&self) -> &StopReason {
        &self.data.stop_reason
    }
}

// ---------------------------------------------------------------------------------------------
// ToolResultEvent

record!(
    /// The record of a [`ToolResultEvent`].
    ToolResultEventData, "toolResultEvent" {
        /// The tool result.
        #[serde(serialize_with = "wrap_tool_result")]
        result: ToolResultBlock,
    }
);

/// Fired when a tool call's result is final (TS `ToolResultEvent`).
pub struct ToolResultEvent {
    agent: ToolAgent,
    invocation_state: InvocationState,
    data: ToolResultEventData,
}

impl EventData for ToolResultEvent {
    type Data = ToolResultEventData;
}
tool_accessors!(ToolResultEvent);

impl ToolResultEvent {
    pub(crate) fn new(agent: ToolAgent, invocation_state: InvocationState, result: ToolResultBlock) -> Self {
        Self {
            agent,
            invocation_state,
            data: ToolResultEventData { result },
        }
    }

    /// The tool result.
    pub fn result(&self) -> &ToolResultBlock {
        &self.data.result
    }
}

// ---------------------------------------------------------------------------------------------
// ToolStreamUpdateEvent

record!(
    /// The record of a [`ToolStreamUpdateEvent`].
    ToolStreamUpdateEventData, "toolStreamUpdateEvent" {
        /// The tool's progress event.
        event: ToolStreamEvent,
    }
);

/// Fired for each progress event a tool streams (TS `ToolStreamUpdateEvent`).
pub struct ToolStreamUpdateEvent {
    agent: ToolAgent,
    invocation_state: InvocationState,
    data: ToolStreamUpdateEventData,
}

impl EventData for ToolStreamUpdateEvent {
    type Data = ToolStreamUpdateEventData;
}
tool_accessors!(ToolStreamUpdateEvent);

impl ToolStreamUpdateEvent {
    pub(crate) fn new(agent: ToolAgent, invocation_state: InvocationState, event: ToolStreamEvent) -> Self {
        Self {
            agent,
            invocation_state,
            data: ToolStreamUpdateEventData { event },
        }
    }

    /// The tool's progress event.
    pub fn event(&self) -> &ToolStreamEvent {
        &self.data.event
    }
}

// ---------------------------------------------------------------------------------------------
// AgentResultEvent

/// Fired with the final result of an invocation (TS `AgentResultEvent`).
pub struct AgentResultEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: AgentResult,
}

impl EventData for AgentResultEvent<'_> {
    type Data = AgentResult;
}
sequential_accessors!(AgentResultEvent);

impl<'a> AgentResultEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        result: AgentResult,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: result,
        }
    }

    /// The result.
    pub fn result(&self) -> &AgentResult {
        &self.data
    }
}

// ---------------------------------------------------------------------------------------------
// InterruptEvent

record!(
    /// The record of an [`InterruptEvent`].
    InterruptEventData, "interruptEvent" {
        /// The raised interrupt.
        interrupt: Interrupt,
    }
);

/// Fired for each interrupt the invocation stops on (TS `InterruptEvent`).
pub struct InterruptEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: InterruptEventData,
}

impl EventData for InterruptEvent<'_> {
    type Data = InterruptEventData;
}
sequential_accessors!(InterruptEvent);

impl<'a> InterruptEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        interrupt: Interrupt,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: InterruptEventData { interrupt },
        }
    }

    /// The raised interrupt.
    pub fn interrupt(&self) -> &Interrupt {
        &self.data.interrupt
    }
}

// ---------------------------------------------------------------------------------------------
// BeforeToolsEvent

/// The record of a [`BeforeToolsEvent`].
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename = "beforeToolsEvent", rename_all = "camelCase")]
#[non_exhaustive]
pub struct BeforeToolsEventData {
    /// The assistant message with the tool calls.
    pub message: Message,
    #[serde(skip)]
    pub(crate) denied: bool,
}

/// Fired before a batch of tool calls runs (TS `BeforeToolsEvent`).
///
/// Hooks may cancel the whole batch or interrupt for approval.
pub struct BeforeToolsEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: BeforeToolsEventData,
    cancel: Option<Cancel>,
    interrupts: Vec<Interrupt>,
}

impl EventData for BeforeToolsEvent<'_> {
    type Data = BeforeToolsEventData;
}
sequential_accessors!(BeforeToolsEvent);

impl<'a> BeforeToolsEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        message: Message,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: BeforeToolsEventData {
                message,
                denied: false,
            },
            cancel: None,
            interrupts: Vec::new(),
        }
    }

    /// The assistant message with the tool calls.
    pub fn message(&self) -> &Message {
        &self.data.message
    }

    /// The cancel request, if any.
    pub fn cancel(&self) -> Option<&Cancel> {
        self.cancel.as_ref()
    }

    /// Cancels every call in the batch; the model sees error results with the message.
    pub fn set_cancel(&mut self, cancel: impl Into<Cancel>) {
        self.cancel = Some(cancel.into());
    }

    /// Withdraws a cancel request.
    pub fn clear_cancel(&mut self) {
        self.cancel = None;
    }

    /// Asks the caller for input before the batch runs (TS `event.interrupt`).
    ///
    /// On resume this returns the caller's response. Otherwise it records the interrupt and
    /// returns `None`; the agent stops with [`StopReason::Interrupt`] once the hooks finish.
    pub fn interrupt(&mut self, params: impl Into<InterruptParams>) -> Option<JsonValue> {
        let params = params.into();
        let id = format!("hook:beforeTools:{}", params.name);
        let raised = self
            .agent
            .interrupt_state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .interrupt(&id, params, InterruptSource::Hook);
        match raised {
            Ok(response) => Some(response),
            Err(interrupt) => {
                self.interrupts.push(*interrupt);
                None
            }
        }
    }

    pub(crate) fn into_parts(mut self) -> (BeforeToolsEventData, Option<String>, Vec<Interrupt>) {
        let cancel = self
            .cancel
            .and_then(|cancel| cancel.resolve("Tool cancelled by hook"));
        self.data.denied = cancel.is_some();
        (self.data, cancel, self.interrupts)
    }
}

// ---------------------------------------------------------------------------------------------
// AfterToolsEvent

record!(
    /// The record of an [`AfterToolsEvent`].
    AfterToolsEventData, "afterToolsEvent" {
        /// The user message with the tool results.
        message: Message,
    }
);

/// Fired after a batch of tool calls (TS `AfterToolsEvent`).
///
/// Setting `end_turn` ends the turn without another model call.
pub struct AfterToolsEvent<'a> {
    agent: &'a mut LocalAgent,
    invocation_state: InvocationState,
    invocation_context: Option<InvocationContext>,
    data: AfterToolsEventData,
    end_turn: Option<EndTurn>,
}

impl EventData for AfterToolsEvent<'_> {
    type Data = AfterToolsEventData;
}
sequential_accessors!(AfterToolsEvent);

impl<'a> AfterToolsEvent<'a> {
    pub(crate) fn new(
        agent: &'a mut LocalAgent,
        invocation_state: InvocationState,
        invocation_context: Option<InvocationContext>,
        message: Message,
    ) -> Self {
        Self {
            agent,
            invocation_state,
            invocation_context,
            data: AfterToolsEventData { message },
            end_turn: None,
        }
    }

    /// The user message with the tool results.
    pub fn message(&self) -> &Message {
        &self.data.message
    }

    /// The end-turn request, if any.
    pub fn end_turn(&self) -> Option<&EndTurn> {
        self.end_turn.as_ref()
    }

    /// Ends the turn after these tools; the value becomes the final assistant message.
    pub fn set_end_turn(&mut self, end_turn: impl Into<EndTurn>) {
        self.end_turn = Some(end_turn.into());
    }

    /// Withdraws an end-turn request.
    pub fn clear_end_turn(&mut self) {
        self.end_turn = None;
    }

    pub(crate) fn take_end_turn(&mut self) -> Option<EndTurn> {
        self.end_turn.take()
    }
}

#[cfg(test)]
mod tests;
