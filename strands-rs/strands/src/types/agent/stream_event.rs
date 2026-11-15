//! The serializable records an agent stream yields (TS `AgentStreamEvent`).
//!
//! Hook callbacks receive the live events in [`crate::hooks`]; the stream yields these owned
//! records instead, built after the hooks ran. Each serializes to the TS event's `toJSON()` shape,
//! so the agent, the invocation state and control fields (`cancel`, `retry`, ...) are left out.
//! The live events serialize the same way, through their record.

use serde::{Serialize, Serializer};

use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, Cancel,
    ContentBlockEvent, InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent, ModelStopData,
    ModelStreamUpdateEvent, ToolResultEvent, ToolStreamUpdateEvent,
};
use crate::interrupt::Interrupt;
use crate::models::streaming::{ContentBlockDelta, ModelStreamEvent};
use crate::tools::ToolStreamEvent;
use crate::types::agent::AgentResult;
use crate::types::messages::{ContentBlock, Message, StopReason, ToolResultBlock, serialize_wrapped};
use crate::types::tools::ToolUse;

/// An item of an agent stream (TS `AgentStreamEvent`).
///
/// Serializes as the TS event's `toJSON()`: `{"type": "<tsType>", ..}`.
///
/// ```
/// use strands::AgentStreamEvent;
///
/// fn print_text(event: &AgentStreamEvent) {
///     if let AgentStreamEvent::ModelStreamUpdate(update) = event {
///         if let Some(text) = update.text_delta() {
///             print!("{text}");
///         }
///     }
/// }
/// ```
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum AgentStreamEvent {
    /// A raw model stream event (TS `ModelStreamUpdateEvent`).
    ModelStreamUpdate(ModelStreamUpdateEventData),
    /// A content block, complete at its stop event (TS `ContentBlockEvent`).
    ContentBlock(ContentBlockEventData),
    /// The model's complete message (TS `ModelMessageEvent`).
    ModelMessage(ModelMessageEventData),
    /// A tool's progress event (TS `ToolStreamUpdateEvent`).
    ToolStreamUpdate(ToolStreamUpdateEventData),
    /// A tool's final result (TS `ToolResultEvent`).
    ToolResult(ToolResultEventData),
    /// The invocation started (TS `BeforeInvocationEvent`).
    BeforeInvocation(BeforeInvocationEventData),
    /// The invocation ended (TS `AfterInvocationEvent`).
    AfterInvocation(AfterInvocationEventData),
    /// A model call is about to start (TS `BeforeModelCallEvent`).
    BeforeModelCall(BeforeModelCallEventData),
    /// A model call ended (TS `AfterModelCallEvent`).
    AfterModelCall(AfterModelCallEventData),
    /// A tool batch is about to run (TS `BeforeToolsEvent`).
    BeforeTools(BeforeToolsEventData),
    /// A tool batch finished (TS `AfterToolsEvent`).
    AfterTools(AfterToolsEventData),
    /// A tool call is about to run (TS `BeforeToolCallEvent`).
    BeforeToolCall(BeforeToolCallEventData),
    /// A tool call finished (TS `AfterToolCallEvent`).
    AfterToolCall(AfterToolCallEventData),
    /// A message was appended to the history (TS `MessageAddedEvent`).
    MessageAdded(MessageAddedEventData),
    /// The invocation raised an interrupt (TS `InterruptEvent`).
    Interrupt(InterruptEventData),
    /// The invocation's result; the last item of a successful stream (TS `AgentResultEvent`).
    AgentResult(AgentResult),
}

#[derive(Serialize)]
#[serde(tag = "type", rename = "agentResultEvent")]
struct AgentResultEventJson<'a> {
    result: &'a AgentResult,
}

impl Serialize for AgentStreamEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::ModelStreamUpdate(data) => data.serialize(serializer),
            Self::ContentBlock(data) => data.serialize(serializer),
            Self::ModelMessage(data) => data.serialize(serializer),
            Self::ToolStreamUpdate(data) => data.serialize(serializer),
            Self::ToolResult(data) => data.serialize(serializer),
            Self::BeforeInvocation(data) => data.serialize(serializer),
            Self::AfterInvocation(data) => data.serialize(serializer),
            Self::BeforeModelCall(data) => data.serialize(serializer),
            Self::AfterModelCall(data) => data.serialize(serializer),
            Self::BeforeTools(data) => data.serialize(serializer),
            Self::AfterTools(data) => data.serialize(serializer),
            Self::BeforeToolCall(data) => data.serialize(serializer),
            Self::AfterToolCall(data) => data.serialize(serializer),
            Self::MessageAdded(data) => data.serialize(serializer),
            Self::Interrupt(data) => data.serialize(serializer),
            Self::AgentResult(result) => AgentResultEventJson { result }.serialize(serializer),
        }
    }
}

fn serialize_tool_result<S: Serializer>(result: &ToolResultBlock, serializer: S) -> Result<S::Ok, S::Error> {
    serialize_wrapped(serializer, "toolResult", result)
}

/// An error as TS `toJSON()` writes it: `{"message": ..}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct EventError {
    message: String,
}

impl EventError {
    fn new(error: &(impl std::fmt::Display + ?Sized)) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

/// A record getter: `ref` borrows the field, `copy` returns it, `opt` borrows the content of an
/// optional field, `none` defines no getter.
macro_rules! getter {
    (ref $field:ident: $ty:ty, $doc:literal) => {
        #[doc = $doc]
        pub fn $field(&self) -> &$ty {
            &self.$field
        }
    };
    (copy $field:ident: $ty:ty, $doc:literal) => {
        #[doc = $doc]
        pub fn $field(&self) -> $ty {
            self.$field
        }
    };
    (opt $field:ident: $ty:ty, $doc:literal) => {
        #[doc = $doc]
        pub fn $field(&self) -> Option<&$ty> {
            self.$field.as_ref()
        }
    };
    (none $field:ident: $ty:ty, $doc:literal) => {};
}

/// A record field's type: `opt` fields hold an `Option`.
macro_rules! field_ty {
    (opt $ty:ty) => { Option<$ty> };
    ($kind:ident $ty:ty) => { $ty };
}

/// Defines a record, its getters and its `From<&live event>` conversion.
macro_rules! record {
    (
        $(#[$meta:meta])*
        $name:ident = $ts:literal from $live:ty {
            $(
                $(#[$field_meta:meta])*
                $kind:ident $field:ident: $ty:ty = |$event:ident| $value:expr, $doc:literal;
            )*
        }
    ) => {
        $(#[$meta])*
        #[derive(Debug, Clone, PartialEq, Serialize)]
        #[serde(tag = "type", rename = $ts, rename_all = "camelCase")]
        #[non_exhaustive]
        pub struct $name {
            $( $(#[$field_meta])* $field: field_ty!($kind $ty), )*
        }

        impl $name {
            $( getter!($kind $field: $ty, $doc); )*
        }

        impl From<&$live> for $name {
            #[allow(unused_variables, reason = "records without fields ignore the event")]
            fn from(live: &$live) -> Self {
                Self { $( $field: { let $event = live; $value }, )* }
            }
        }
    };
}

record! {
    /// The payload of [`AgentStreamEvent::ModelStreamUpdate`] (TS `ModelStreamUpdateEvent`).
    ModelStreamUpdateEventData = "modelStreamUpdateEvent" from ModelStreamUpdateEvent<'_> {
        ref event: ModelStreamEvent = |e| e.event().clone(), "The model stream event.";
    }
}

impl ModelStreamUpdateEventData {
    /// The text of a text delta event, if this is one.
    pub fn text_delta(&self) -> Option<&str> {
        let ModelStreamEvent::BlockDelta(delta) = &self.event else {
            return None;
        };
        match delta.delta() {
            ContentBlockDelta::Text(text) => Some(text.text()),
            _ => None,
        }
    }
}

record! {
    /// The payload of [`AgentStreamEvent::ContentBlock`] (TS `ContentBlockEvent`).
    ContentBlockEventData = "contentBlockEvent" from ContentBlockEvent<'_> {
        ref content_block: ContentBlock = |e| e.content_block().clone(), "The completed content block.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::ModelMessage`] (TS `ModelMessageEvent`).
    ModelMessageEventData = "modelMessageEvent" from ModelMessageEvent<'_> {
        ref message: Message = |e| e.message().clone(), "The model's message.";
        ref stop_reason: StopReason = |e| e.stop_reason().clone(), "Why the model stopped.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::ToolStreamUpdate`] (TS `ToolStreamUpdateEvent`).
    ToolStreamUpdateEventData = "toolStreamUpdateEvent" from ToolStreamUpdateEvent {
        ref event: ToolStreamEvent = |e| e.event().clone(), "The tool's progress event.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::ToolResult`] (TS `ToolResultEvent`).
    ToolResultEventData = "toolResultEvent" from ToolResultEvent {
        #[serde(serialize_with = "serialize_tool_result")]
        ref result: ToolResultBlock = |e| e.result().clone(), "The tool result.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::BeforeInvocation`] (TS `BeforeInvocationEvent`).
    BeforeInvocationEventData = "beforeInvocationEvent" from BeforeInvocationEvent<'_> {}
}

record! {
    /// The payload of [`AgentStreamEvent::AfterInvocation`] (TS `AfterInvocationEvent`).
    AfterInvocationEventData = "afterInvocationEvent" from AfterInvocationEvent<'_> {}
}

record! {
    /// The payload of [`AgentStreamEvent::BeforeModelCall`] (TS `BeforeModelCallEvent`).
    BeforeModelCallEventData = "beforeModelCallEvent" from BeforeModelCallEvent<'_> {
        #[serde(skip_serializing_if = "Option::is_none")]
        copy projected_input_tokens: Option<u64> = |e| e.projected_input_tokens(),
            "The estimated input tokens of the call, when estimation succeeded.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::AfterModelCall`] (TS `AfterModelCallEvent`).
    AfterModelCallEventData = "afterModelCallEvent" from AfterModelCallEvent<'_> {
        copy attempt_count: u32 = |e| e.attempt_count().get(), "The attempt number of this call, from 1.";
        #[serde(skip_serializing_if = "Option::is_none")]
        opt stop_data: ModelStopData = |e| e.stop_data().cloned(), "What a successful call produced.";
        #[serde(skip_serializing_if = "Option::is_none")]
        none error: Option<EventError> = |e| e.error().map(EventError::new), "";
    }
}

impl AfterModelCallEventData {
    /// The error message of a failed call.
    pub fn error_message(&self) -> Option<&str> {
        self.error.as_ref().map(|error| error.message.as_str())
    }
}

record! {
    /// The payload of [`AgentStreamEvent::BeforeTools`] (TS `BeforeToolsEvent`).
    BeforeToolsEventData = "beforeToolsEvent" from BeforeToolsEvent<'_> {
        ref message: Message = |e| e.message().clone(), "The assistant message requesting the tools.";
        #[serde(skip)]
        opt cancel: Cancel = |e| e.cancel().cloned(), "The batch cancel hooks requested, if any; not serialized.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::AfterTools`] (TS `AfterToolsEvent`).
    AfterToolsEventData = "afterToolsEvent" from AfterToolsEvent<'_> {
        ref message: Message = |e| e.message().clone(), "The user message carrying the tool results.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::BeforeToolCall`] (TS `BeforeToolCallEvent`).
    BeforeToolCallEventData = "beforeToolCallEvent" from BeforeToolCallEvent {
        ref tool_use: ToolUse = |e| e.tool_use().clone(), "The tool call, after the hooks ran.";
        #[serde(skip)]
        opt cancel: Cancel = |e| e.cancel().cloned(), "The cancel hooks requested, if any; not serialized.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::AfterToolCall`] (TS `AfterToolCallEvent`).
    AfterToolCallEventData = "afterToolCallEvent" from AfterToolCallEvent {
        ref tool_use: ToolUse = |e| e.tool_use().clone(), "The tool call.";
        #[serde(serialize_with = "serialize_tool_result")]
        ref result: ToolResultBlock = |e| e.result().clone(), "The tool result, after the hooks ran.";
        #[serde(skip_serializing_if = "Option::is_none")]
        none error: Option<EventError> = |e| e.error().map(EventError::new), "";
    }
}

impl AfterToolCallEventData {
    /// The message of the error behind an error result.
    pub fn error_message(&self) -> Option<&str> {
        self.error.as_ref().map(|error| error.message.as_str())
    }
}

record! {
    /// The payload of [`AgentStreamEvent::MessageAdded`] (TS `MessageAddedEvent`).
    MessageAddedEventData = "messageAddedEvent" from MessageAddedEvent<'_> {
        ref message: Message = |e| e.message().clone(), "The appended message.";
    }
}

record! {
    /// The payload of [`AgentStreamEvent::Interrupt`] (TS `InterruptEvent`).
    InterruptEventData = "interruptEvent" from InterruptEvent<'_> {
        ref interrupt: Interrupt = |e| e.interrupt().clone(), "The raised interrupt.";
    }
}

/// Live hook events serialize as their TS `toJSON()`, through their record.
macro_rules! serialize_live {
    ($($live:ty => $record:ty),* $(,)?) => {
        $(
            impl Serialize for $live {
                fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
                    <$record>::from(self).serialize(serializer)
                }
            }
        )*
    };
}

serialize_live! {
    ModelStreamUpdateEvent<'_> => ModelStreamUpdateEventData,
    ContentBlockEvent<'_> => ContentBlockEventData,
    ModelMessageEvent<'_> => ModelMessageEventData,
    ToolStreamUpdateEvent => ToolStreamUpdateEventData,
    ToolResultEvent => ToolResultEventData,
    BeforeInvocationEvent<'_> => BeforeInvocationEventData,
    AfterInvocationEvent<'_> => AfterInvocationEventData,
    BeforeModelCallEvent<'_> => BeforeModelCallEventData,
    AfterModelCallEvent<'_> => AfterModelCallEventData,
    BeforeToolsEvent<'_> => BeforeToolsEventData,
    AfterToolsEvent<'_> => AfterToolsEventData,
    BeforeToolCallEvent => BeforeToolCallEventData,
    AfterToolCallEvent => AfterToolCallEventData,
    MessageAddedEvent<'_> => MessageAddedEventData,
    InterruptEvent<'_> => InterruptEventData,
}

impl Serialize for AgentResultEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        AgentResultEventJson {
            result: self.result(),
        }
        .serialize(serializer)
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename = "initializedEvent")]
struct InitializedEventJson {}

impl Serialize for InitializedEvent<'_> {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        InitializedEventJson {}.serialize(serializer)
    }
}

#[cfg(test)]
mod tests;
