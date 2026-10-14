//! Hook events (`strands-ts/src/hooks/events.ts`). One struct per event; mutable fields are the
//! control surface callbacks write and the agent loop reads afterwards.

use std::sync::Arc;

use serde::Serialize;

use super::registry::HookEvent;
use crate::agent::{Agent, AgentResult, InvocationState, InvokeInput};
use crate::error::Error;
use crate::models::{Model, ModelStreamEvent};
use crate::tools::{ToolRef, ToolStreamEvent};
use crate::types::{ContentBlock, Message, StopReason, ToolResultBlock, ToolUse};

/// Why a cancellable step was cancelled: `false` (not cancelled), `true`, or a message.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Cancel {
    /// Proceed.
    #[default]
    No,
    /// Cancel with the default message.
    Yes,
    /// Cancel with a custom message.
    Reason(String),
}

impl Cancel {
    /// `true` unless `No`.
    pub fn is_cancelled(&self) -> bool {
        !matches!(self, Cancel::No)
    }

    /// Message to use, or `default` for a bare `Yes`.
    pub fn message_or<'a>(&'a self, default: &'a str) -> Option<&'a str> {
        match self {
            Cancel::No => None,
            Cancel::Yes => Some(default),
            Cancel::Reason(r) => Some(r),
        }
    }
}

impl From<bool> for Cancel {
    fn from(b: bool) -> Self {
        if b {
            Cancel::Yes
        } else {
            Cancel::No
        }
    }
}

impl From<&str> for Cancel {
    fn from(s: &str) -> Self {
        Cancel::Reason(s.to_owned())
    }
}

impl From<String> for Cancel {
    fn from(s: String) -> Self {
        Cancel::Reason(s)
    }
}

pub use crate::models::Redaction;

/// The model's final message and stop reason.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStopData {
    /// The assistant message.
    pub message: Message,
    /// Stop reason.
    pub stop_reason: StopReason,
    /// Input redaction, if any.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redaction: Option<Redaction>,
}

/// Agent construction finished.
#[derive(Clone)]
pub struct InitializedEvent {
    /// The agent.
    pub agent: Agent,
}
impl HookEvent for InitializedEvent {}

/// An invocation is starting. Set `cancel` to abort.
#[derive(Clone)]
pub struct BeforeInvocationEvent {
    /// The agent.
    pub agent: Agent,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// Cancel the invocation.
    pub cancel: Cancel,
}
impl HookEvent for BeforeInvocationEvent {}

/// An invocation finished. Set `resume` to immediately start another one.
#[derive(Clone)]
pub struct AfterInvocationEvent {
    /// The agent.
    pub agent: Agent,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// Input for a follow-up invocation.
    pub resume: Option<InvokeInput>,
}
impl HookEvent for AfterInvocationEvent {
    fn reverse_callbacks() -> bool {
        true
    }
}

/// A message was appended to `agent.messages`.
#[derive(Clone)]
pub struct MessageAddedEvent {
    /// The agent.
    pub agent: Agent,
    /// The message.
    pub message: Message,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
}
impl HookEvent for MessageAddedEvent {}

/// A tool is about to run. `tool_use` and `selected_tool` are writable.
#[derive(Clone)]
pub struct BeforeToolCallEvent {
    /// The agent.
    pub agent: Agent,
    /// The request (mutable: callbacks may rewrite it).
    pub tool_use: ToolUse,
    /// The tool resolved by name, if any.
    pub tool: Option<ToolRef>,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// Cancel this tool call; the message becomes the error result.
    pub cancel: Cancel,
    /// Override which tool runs.
    pub selected_tool: Option<ToolRef>,
}
impl HookEvent for BeforeToolCallEvent {}

impl BeforeToolCallEvent {
    /// Ask the caller for input before this tool runs: the answer on resume, otherwise
    /// [`Error::Interrupt`](crate::error::Error::Interrupt).
    pub fn interrupt<T: serde::de::DeserializeOwned>(
        &self,
        params: crate::types::InterruptParams,
    ) -> crate::error::Result<T> {
        let id = crate::interrupt::before_tool_call_interrupt_id(&self.tool_use.tool_use_id, &params.name);
        let response = crate::interrupt::raise(
            &self.agent.interrupt_state_shared(),
            &id,
            params,
            crate::interrupt::InterruptSource::Hook,
        )?;
        serde_json::from_value(response).map_err(|e| Error::other(e.to_string()))
    }
}

/// A tool finished. `result` is replaceable; set `retry` to run it again.
#[derive(Clone)]
pub struct AfterToolCallEvent {
    /// The agent.
    pub agent: Agent,
    /// The request.
    pub tool_use: ToolUse,
    /// The tool that ran, if any.
    pub tool: Option<ToolRef>,
    /// The result (mutable).
    pub result: ToolResultBlock,
    /// Error that produced an error result, if any.
    pub error: Option<Arc<Error>>,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// Re-run the tool.
    pub retry: bool,
}
impl HookEvent for AfterToolCallEvent {
    fn reverse_callbacks() -> bool {
        true
    }
}

/// The model is about to be called.
#[derive(Clone)]
pub struct BeforeModelCallEvent {
    /// The agent.
    pub agent: Agent,
    /// The model.
    pub model: Arc<dyn Model>,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// Estimated prompt tokens, if computed.
    pub projected_input_tokens: Option<u64>,
    /// Cancel the invocation.
    pub cancel: Cancel,
    /// The messages that will be sent to the model (a copy of the conversation). Callbacks may
    /// edit this for the current call only; `agent.messages()` is unaffected. Replaces TS's
    /// `InvokeModelStage.Input` middleware seam.
    pub messages: Vec<Message>,
}
impl HookEvent for BeforeModelCallEvent {}

/// The model call finished (successfully or not). Set `retry` to call again.
#[derive(Clone)]
pub struct AfterModelCallEvent {
    /// The agent.
    pub agent: Agent,
    /// The model.
    pub model: Arc<dyn Model>,
    /// Final message + stop reason on success.
    pub stop_data: Option<ModelStopData>,
    /// Error on failure.
    pub error: Option<Arc<Error>>,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// 1-based attempt number.
    pub attempt_count: u32,
    /// Retry the model call.
    pub retry: bool,
}
impl HookEvent for AfterModelCallEvent {
    fn reverse_callbacks() -> bool {
        true
    }
}

/// A raw model stream event.
#[derive(Clone)]
pub struct ModelStreamUpdateEvent {
    /// The agent.
    pub agent: Agent,
    /// The provider event.
    pub event: ModelStreamEvent,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
}
impl HookEvent for ModelStreamUpdateEvent {}

/// A fully assembled content block.
#[derive(Clone)]
pub struct ContentBlockEvent {
    /// The agent.
    pub agent: Agent,
    /// The block.
    pub content_block: ContentBlock,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
}
impl HookEvent for ContentBlockEvent {}

/// The model produced a complete message.
#[derive(Clone)]
pub struct ModelMessageEvent {
    /// The agent.
    pub agent: Agent,
    /// The message.
    pub message: Message,
    /// Stop reason.
    pub stop_reason: StopReason,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
}
impl HookEvent for ModelMessageEvent {}

/// A tool produced its result.
#[derive(Clone)]
pub struct ToolResultEvent {
    /// The agent.
    pub agent: Agent,
    /// The result.
    pub result: ToolResultBlock,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
}
impl HookEvent for ToolResultEvent {}

/// A tool emitted progress.
#[derive(Clone)]
pub struct ToolStreamUpdateEvent {
    /// The agent.
    pub agent: Agent,
    /// The progress event.
    pub event: ToolStreamEvent,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
}
impl HookEvent for ToolStreamUpdateEvent {}

/// The invocation produced its result.
#[derive(Clone)]
pub struct AgentResultEvent {
    /// The agent.
    pub agent: Agent,
    /// The result.
    pub result: AgentResult,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
}
impl HookEvent for AgentResultEvent {}

/// A batch of tool calls is about to run. Set `cancel` to skip them all.
#[derive(Clone)]
pub struct BeforeToolsEvent {
    /// The agent.
    pub agent: Agent,
    /// The assistant message containing the tool uses.
    pub message: Message,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// Cancel every tool call in the batch.
    pub cancel: Cancel,
}
impl HookEvent for BeforeToolsEvent {}

impl BeforeToolsEvent {
    /// Ask the caller for input before the whole batch runs.
    pub fn interrupt<T: serde::de::DeserializeOwned>(
        &self,
        params: crate::types::InterruptParams,
    ) -> crate::error::Result<T> {
        let id = crate::interrupt::before_tools_interrupt_id(&params.name);
        let response = crate::interrupt::raise(
            &self.agent.interrupt_state_shared(),
            &id,
            params,
            crate::interrupt::InterruptSource::Hook,
        )?;
        serde_json::from_value(response).map_err(|e| Error::other(e.to_string()))
    }
}

/// How `AfterToolsEvent` may end the turn.
#[derive(Clone, Debug, PartialEq, Default)]
pub enum EndTurn {
    /// Continue the loop.
    #[default]
    No,
    /// End the turn with the tool results as the last message.
    Yes,
    /// End the turn with an extra assistant text message.
    Text(String),
    /// End the turn with an extra assistant message of these blocks.
    Blocks(Vec<ContentBlock>),
}

/// A batch of tool calls finished. Set `end_turn` to stop looping.
#[derive(Clone)]
pub struct AfterToolsEvent {
    /// The agent.
    pub agent: Agent,
    /// The user message containing the tool results.
    pub message: Message,
    /// Shared invocation state.
    pub invocation_state: InvocationState,
    /// Stop instead of calling the model again.
    pub end_turn: EndTurn,
}
impl HookEvent for AfterToolsEvent {
    fn reverse_callbacks() -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cancel_conversions() {
        assert!(!Cancel::from(false).is_cancelled());
        assert_eq!(Cancel::from(true).message_or("d"), Some("d"));
        assert_eq!(Cancel::from("why").message_or("d"), Some("why"));
        assert_eq!(Cancel::No.message_or("d"), None);
    }

    #[test]
    fn reverse_flags() {
        assert!(AfterInvocationEvent::reverse_callbacks());
        assert!(AfterToolCallEvent::reverse_callbacks());
        assert!(AfterModelCallEvent::reverse_callbacks());
        assert!(AfterToolsEvent::reverse_callbacks());
        assert!(!BeforeToolCallEvent::reverse_callbacks());
        assert!(!MessageAddedEvent::reverse_callbacks());
    }
}
