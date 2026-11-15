//! Owned, serializable records of what an agent stream yields (TS `AgentStreamEvent`, the stream
//! side of the event classes in `hooks/events.ts`).
//!
//! Hook callbacks receive the live hook events; the stream yields these records instead. Each
//! record serializes to the TS event's `toJSON()` shape, so it holds no agent, invocation state or
//! control fields (`cancel`, `retry`, `resume`, `endTurn`, `selectedTool`), and errors appear as
//! `{"message": ..}`.

use serde::{Deserialize, Serialize};

use crate::interrupt::Interrupt;
use crate::models::model::Redaction;
use crate::models::streaming::ModelStreamEvent;
use crate::tools::ToolStreamEvent;
use crate::types::agent::AgentResult;
use crate::types::messages::{ContentBlock, Message, StopReason, ToolResultBlock};
use crate::types::tools::ToolUse;

/// An item of an agent stream (TS `AgentStreamEvent`), tagged with the TS event `type`.
///
/// ```
/// use strands::AgentStreamEvent;
/// use strands::agent::stream_event::ModelStreamUpdateEventData;
/// use strands::models::streaming::{ModelStreamEvent, TextDelta};
///
/// let event = AgentStreamEvent::ModelStreamUpdate(ModelStreamUpdateEventData::new(
///     ModelStreamEvent::from(TextDelta::new("Hi")),
/// ));
/// if let AgentStreamEvent::ModelStreamUpdate(update) = &event {
///     assert_eq!(update.text_delta(), Some("Hi"));
/// }
/// assert_eq!(
///     serde_json::to_value(&event).unwrap(),
///     serde_json::json!({
///         "type": "modelStreamUpdateEvent",
///         "event": { "type": "modelContentBlockDeltaEvent", "delta": { "type": "textDelta", "text": "Hi" } }
///     })
/// );
/// ```
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AgentStreamEvent {
    /// A model stream event (TS `ModelStreamUpdateEvent`).
    #[serde(rename = "modelStreamUpdateEvent")]
    ModelStreamUpdate(ModelStreamUpdateEventData),
    /// A completed content block (TS `ContentBlockEvent`).
    #[serde(rename = "contentBlockEvent")]
    ContentBlock(ContentBlockEventData),
    /// The model's complete message (TS `ModelMessageEvent`).
    #[serde(rename = "modelMessageEvent")]
    ModelMessage(ModelMessageEventData),
    /// A tool's progress event (TS `ToolStreamUpdateEvent`).
    #[serde(rename = "toolStreamUpdateEvent")]
    ToolStreamUpdate(ToolStreamUpdateEventData),
    /// A tool's result (TS `ToolResultEvent`).
    #[serde(rename = "toolResultEvent")]
    ToolResult(ToolResultEventData),
    /// The invocation started (TS `BeforeInvocationEvent`).
    #[serde(rename = "beforeInvocationEvent")]
    BeforeInvocation(BeforeInvocationEventData),
    /// The invocation finished (TS `AfterInvocationEvent`).
    #[serde(rename = "afterInvocationEvent")]
    AfterInvocation(AfterInvocationEventData),
    /// A model call is about to start (TS `BeforeModelCallEvent`).
    #[serde(rename = "beforeModelCallEvent")]
    BeforeModelCall(BeforeModelCallEventData),
    /// A model call finished or failed (TS `AfterModelCallEvent`).
    #[serde(rename = "afterModelCallEvent")]
    AfterModelCall(AfterModelCallEventData),
    /// A batch of tool calls is about to run (TS `BeforeToolsEvent`).
    #[serde(rename = "beforeToolsEvent")]
    BeforeTools(BeforeToolsEventData),
    /// A batch of tool calls finished (TS `AfterToolsEvent`).
    #[serde(rename = "afterToolsEvent")]
    AfterTools(AfterToolsEventData),
    /// A tool call is about to run (TS `BeforeToolCallEvent`).
    #[serde(rename = "beforeToolCallEvent")]
    BeforeToolCall(BeforeToolCallEventData),
    /// A tool call finished (TS `AfterToolCallEvent`).
    #[serde(rename = "afterToolCallEvent")]
    AfterToolCall(AfterToolCallEventData),
    /// A message was added to the conversation (TS `MessageAddedEvent`).
    #[serde(rename = "messageAddedEvent")]
    MessageAdded(MessageAddedEventData),
    /// An interrupt was raised (TS `InterruptEvent`).
    #[serde(rename = "interruptEvent")]
    Interrupt(InterruptEventData),
    /// The invocation's result, the last item of a successful stream (TS `AgentResultEvent`),
    /// written as `{"type":"agentResultEvent","result":..}`.
    #[serde(rename = "agentResultEvent", with = "agent_result_event")]
    AgentResult(AgentResult),
}

mod agent_result_event {
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    use crate::types::agent::AgentResult;

    #[derive(Serialize)]
    struct Borrowed<'a> {
        result: &'a AgentResult,
    }

    #[derive(Deserialize)]
    struct Owned {
        result: AgentResult,
    }

    pub(super) fn serialize<S: Serializer>(result: &AgentResult, serializer: S) -> Result<S::Ok, S::Error> {
        Borrowed { result }.serialize(serializer)
    }

    pub(super) fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<AgentResult, D::Error> {
        Owned::deserialize(deserializer).map(|owned| owned.result)
    }
}

/// Data of [`AgentStreamEvent::ModelStreamUpdate`] (TS `ModelStreamUpdateEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelStreamUpdateEventData {
    /// The model stream event.
    pub event: ModelStreamEvent,
}

impl ModelStreamUpdateEventData {
    /// Wraps a model stream event.
    #[must_use]
    pub fn new(event: ModelStreamEvent) -> Self {
        Self { event }
    }

    /// The text of a text delta, if the event is one.
    #[must_use]
    pub fn text_delta(&self) -> Option<&str> {
        self.event.text_delta()
    }
}

/// Data of [`AgentStreamEvent::ContentBlock`] (TS `ContentBlockEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentBlockEventData {
    /// The completed block.
    pub content_block: ContentBlock,
}

impl ContentBlockEventData {
    /// Wraps a completed block.
    #[must_use]
    pub fn new(content_block: ContentBlock) -> Self {
        Self { content_block }
    }
}

/// Data of [`AgentStreamEvent::ModelMessage`] (TS `ModelMessageEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelMessageEventData {
    /// The model's message.
    pub message: Message,
    /// Why the model stopped.
    pub stop_reason: StopReason,
}

impl ModelMessageEventData {
    /// Creates the data.
    #[must_use]
    pub fn new(message: Message, stop_reason: StopReason) -> Self {
        Self { message, stop_reason }
    }
}

/// Data of [`AgentStreamEvent::ToolStreamUpdate`] (TS `ToolStreamUpdateEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolStreamUpdateEventData {
    /// The tool's progress event.
    pub event: ToolStreamEvent,
}

impl ToolStreamUpdateEventData {
    /// Wraps a tool progress event.
    #[must_use]
    pub fn new(event: ToolStreamEvent) -> Self {
        Self { event }
    }
}

/// Data of [`AgentStreamEvent::ToolResult`] (TS `ToolResultEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolResultEventData {
    /// The tool result.
    pub result: ToolResultBlock,
}

impl ToolResultEventData {
    /// Wraps a tool result.
    #[must_use]
    pub fn new(result: ToolResultBlock) -> Self {
        Self { result }
    }
}

/// Data of [`AgentStreamEvent::BeforeInvocation`] (TS `BeforeInvocationEvent`), written as
/// `{"type"}` only.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeforeInvocationEventData {}

impl BeforeInvocationEventData {
    /// Creates the data.
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

/// Data of [`AgentStreamEvent::AfterInvocation`] (TS `AfterInvocationEvent`), written as
/// `{"type"}` only.
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AfterInvocationEventData {}

impl AfterInvocationEventData {
    /// Creates the data.
    #[must_use]
    pub fn new() -> Self {
        Self {}
    }
}

/// Data of [`AgentStreamEvent::BeforeModelCall`] (TS `BeforeModelCallEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeModelCallEventData {
    /// The estimated input tokens of the call, when the agent projected them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projected_input_tokens: Option<u64>,
}

impl BeforeModelCallEventData {
    /// Creates the data without a projection.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Sets the projected input tokens.
    #[must_use]
    pub fn with_projected_input_tokens(mut self, tokens: u64) -> Self {
        self.projected_input_tokens = Some(tokens);
        self
    }
}

/// Data of [`AgentStreamEvent::AfterModelCall`] (TS `AfterModelCallEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterModelCallEventData {
    /// The 1-based attempt number of the call.
    pub attempt_count: u32,
    /// The model's response, absent when the call failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_data: Option<ModelStopData>,
    /// The failure, when the call failed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EventError>,
}

impl AfterModelCallEventData {
    /// Creates the data of a successful call.
    #[must_use]
    pub fn new(attempt_count: u32, stop_data: ModelStopData) -> Self {
        Self {
            attempt_count,
            stop_data: Some(stop_data),
            error: None,
        }
    }

    /// Creates the data of a failed call.
    #[must_use]
    pub fn failed(attempt_count: u32, error: EventError) -> Self {
        Self {
            attempt_count,
            stop_data: None,
            error: Some(error),
        }
    }
}

/// A model's response as an [`AfterModelCallEventData`] carries it (TS `ModelStopData`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelStopData {
    /// The model's message.
    pub message: Message,
    /// Why the model stopped.
    pub stop_reason: StopReason,
    /// The input redaction a guardrail requested, if any.
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

/// An error as a record carries it, `{"message": ..}` (the TS `toJSON()` form of an `Error`).
#[non_exhaustive]
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventError {
    /// The error message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl EventError {
    /// Creates the record of an error with the given message.
    #[must_use]
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
        }
    }
}

/// Data of [`AgentStreamEvent::BeforeTools`] (TS `BeforeToolsEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeforeToolsEventData {
    /// The assistant message holding the tool uses.
    pub message: Message,
}

impl BeforeToolsEventData {
    /// Creates the data.
    #[must_use]
    pub fn new(message: Message) -> Self {
        Self { message }
    }
}

/// Data of [`AgentStreamEvent::AfterTools`] (TS `AfterToolsEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AfterToolsEventData {
    /// The user message holding the tool results.
    pub message: Message,
}

impl AfterToolsEventData {
    /// Creates the data.
    #[must_use]
    pub fn new(message: Message) -> Self {
        Self { message }
    }
}

/// Data of [`AgentStreamEvent::BeforeToolCall`] (TS `BeforeToolCallEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeToolCallEventData {
    /// The tool call, after any hook rewrote it (TS `ToolUseData`).
    pub tool_use: ToolUse,
}

impl BeforeToolCallEventData {
    /// Creates the data.
    #[must_use]
    pub fn new(tool_use: ToolUse) -> Self {
        Self { tool_use }
    }
}

/// Data of [`AgentStreamEvent::AfterToolCall`] (TS `AfterToolCallEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AfterToolCallEventData {
    /// The tool call (TS `ToolUseData`).
    pub tool_use: ToolUse,
    /// The tool result.
    pub result: ToolResultBlock,
    /// The error the tool raised, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<EventError>,
}

impl AfterToolCallEventData {
    /// Creates the data without an error.
    #[must_use]
    pub fn new(tool_use: ToolUse, result: ToolResultBlock) -> Self {
        Self {
            tool_use,
            result,
            error: None,
        }
    }

    /// Sets the error.
    #[must_use]
    pub fn with_error(mut self, error: EventError) -> Self {
        self.error = Some(error);
        self
    }
}

/// Data of [`AgentStreamEvent::MessageAdded`] (TS `MessageAddedEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MessageAddedEventData {
    /// The added message.
    pub message: Message,
}

impl MessageAddedEventData {
    /// Creates the data.
    #[must_use]
    pub fn new(message: Message) -> Self {
        Self { message }
    }
}

/// Data of [`AgentStreamEvent::Interrupt`] (TS `InterruptEvent`).
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct InterruptEventData {
    /// The raised interrupt.
    pub interrupt: Interrupt,
}

impl InterruptEventData {
    /// Creates the data.
    #[must_use]
    pub fn new(interrupt: Interrupt) -> Self {
        Self { interrupt }
    }
}
