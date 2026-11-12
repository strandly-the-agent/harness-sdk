//! The agent stream: owned, serializable event records (TS `AgentStreamEvent`).

use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll};

use futures::Stream;
use serde::{Serialize, Serializer};

use crate::agent::AgentResult;
use crate::agent::printer::AgentPrinter;
use crate::error::Result;
use crate::hooks::{
    AfterInvocationEventData, AfterModelCallEventData, AfterToolCallEventData, AfterToolsEventData,
    BeforeInvocationEventData, BeforeModelCallEventData, BeforeToolCallEventData, BeforeToolsEventData,
    ContentBlockEventData, InterruptEventData, MessageAddedEventData, ModelMessageEventData,
    ModelStreamUpdateEventData, ToolResultEventData, ToolStreamUpdateEventData,
};

/// One event of an agent stream (TS `AgentStreamEvent`).
///
/// Each variant is the record of the hook event of the same name, taken after its hooks ran.
/// `Serialize` writes the TS `toJSON()` shape.
#[derive(Debug, Clone)]
#[non_exhaustive]
#[expect(clippy::large_enum_variant, reason = "the result is yielded once, at the end")]
pub enum AgentStreamEvent {
    /// See [`ModelStreamUpdateEvent`](crate::hooks::ModelStreamUpdateEvent).
    ModelStreamUpdate(ModelStreamUpdateEventData),
    /// See [`ContentBlockEvent`](crate::hooks::ContentBlockEvent).
    ContentBlock(ContentBlockEventData),
    /// See [`ModelMessageEvent`](crate::hooks::ModelMessageEvent).
    ModelMessage(ModelMessageEventData),
    /// See [`ToolStreamUpdateEvent`](crate::hooks::ToolStreamUpdateEvent).
    ToolStreamUpdate(ToolStreamUpdateEventData),
    /// See [`ToolResultEvent`](crate::hooks::ToolResultEvent).
    ToolResult(ToolResultEventData),
    /// See [`BeforeInvocationEvent`](crate::hooks::BeforeInvocationEvent).
    BeforeInvocation(BeforeInvocationEventData),
    /// See [`AfterInvocationEvent`](crate::hooks::AfterInvocationEvent).
    AfterInvocation(AfterInvocationEventData),
    /// See [`BeforeModelCallEvent`](crate::hooks::BeforeModelCallEvent).
    BeforeModelCall(BeforeModelCallEventData),
    /// See [`AfterModelCallEvent`](crate::hooks::AfterModelCallEvent).
    AfterModelCall(AfterModelCallEventData),
    /// See [`BeforeToolsEvent`](crate::hooks::BeforeToolsEvent).
    BeforeTools(BeforeToolsEventData),
    /// See [`AfterToolsEvent`](crate::hooks::AfterToolsEvent).
    AfterTools(AfterToolsEventData),
    /// See [`BeforeToolCallEvent`](crate::hooks::BeforeToolCallEvent).
    BeforeToolCall(BeforeToolCallEventData),
    /// See [`AfterToolCallEvent`](crate::hooks::AfterToolCallEvent).
    AfterToolCall(AfterToolCallEventData),
    /// See [`MessageAddedEvent`](crate::hooks::MessageAddedEvent).
    MessageAdded(MessageAddedEventData),
    /// See [`InterruptEvent`](crate::hooks::InterruptEvent).
    Interrupt(InterruptEventData),
    /// The final result; always the last event of a successful stream.
    AgentResult(AgentResult),
}

impl AgentStreamEvent {
    /// The TS `type` tag of this event.
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::ModelStreamUpdate(_) => "modelStreamUpdateEvent",
            Self::ContentBlock(_) => "contentBlockEvent",
            Self::ModelMessage(_) => "modelMessageEvent",
            Self::ToolStreamUpdate(_) => "toolStreamUpdateEvent",
            Self::ToolResult(_) => "toolResultEvent",
            Self::BeforeInvocation(_) => "beforeInvocationEvent",
            Self::AfterInvocation(_) => "afterInvocationEvent",
            Self::BeforeModelCall(_) => "beforeModelCallEvent",
            Self::AfterModelCall(_) => "afterModelCallEvent",
            Self::BeforeTools(_) => "beforeToolsEvent",
            Self::AfterTools(_) => "afterToolsEvent",
            Self::BeforeToolCall(_) => "beforeToolCallEvent",
            Self::AfterToolCall(_) => "afterToolCallEvent",
            Self::MessageAdded(_) => "messageAddedEvent",
            Self::Interrupt(_) => "interruptEvent",
            Self::AgentResult(_) => "agentResultEvent",
        }
    }
}

impl Serialize for AgentStreamEvent {
    fn serialize<S: Serializer>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(tag = "type", rename = "agentResultEvent")]
        struct ResultEvent<'a> {
            result: &'a AgentResult,
        }
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
            Self::AgentResult(result) => ResultEvent { result }.serialize(serializer),
        }
    }
}

macro_rules! into_stream_event {
    ($($data:ident => $variant:ident),* $(,)?) => {$(
        impl From<$data> for AgentStreamEvent {
            fn from(data: $data) -> Self {
                Self::$variant(data)
            }
        }
    )*};
}

into_stream_event!(
    ModelStreamUpdateEventData => ModelStreamUpdate,
    ContentBlockEventData => ContentBlock,
    ModelMessageEventData => ModelMessage,
    ToolStreamUpdateEventData => ToolStreamUpdate,
    ToolResultEventData => ToolResult,
    BeforeInvocationEventData => BeforeInvocation,
    AfterInvocationEventData => AfterInvocation,
    BeforeModelCallEventData => BeforeModelCall,
    AfterModelCallEventData => AfterModelCall,
    BeforeToolsEventData => BeforeTools,
    AfterToolsEventData => AfterTools,
    BeforeToolCallEventData => BeforeToolCall,
    AfterToolCallEventData => AfterToolCall,
    MessageAddedEventData => MessageAdded,
    InterruptEventData => Interrupt,
    AgentResult => AgentResult,
);

type Slot = Arc<Mutex<Option<AgentStreamEvent>>>;

/// Hands events from the loop to the stream one at a time: [`Emitter::emit`] parks the loop
/// until the consumer has taken the event, so the loop runs only as the stream is polled, like
/// a TS async generator.
#[derive(Clone)]
pub(crate) struct Emitter {
    slot: Slot,
    printer: Option<Arc<Mutex<AgentPrinter>>>,
}

impl Emitter {
    pub(crate) fn emit(&self, event: impl Into<AgentStreamEvent>) -> Emit {
        let event = event.into();
        self.print(&event);
        Emit {
            slot: Arc::clone(&self.slot),
            event: Some(event),
        }
    }

    /// Feeds an event to the printer without yielding it.
    pub(crate) fn print(&self, event: &AgentStreamEvent) {
        if let Some(printer) = &self.printer {
            printer
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .process_event(event);
        }
    }
}

pub(crate) struct Emit {
    slot: Slot,
    event: Option<AgentStreamEvent>,
}

impl Future for Emit {
    type Output = ();

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<()> {
        let this = &mut *self;
        let mut slot = this.slot.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(event) = this.event.take() {
            *slot = Some(event);
            return Poll::Pending;
        }
        if slot.is_some() {
            return Poll::Pending;
        }
        Poll::Ready(())
    }
}

type LoopFuture<'a> = Pin<Box<dyn Future<Output = Result<AgentResult>> + Send + 'a>>;

/// The stream of an invocation (TS `agent.stream()`), borrowing the agent.
///
/// Items are `Result<AgentStreamEvent>`; the last item of a successful run is
/// [`AgentStreamEvent::AgentResult`]. The loop advances only while the stream is polled.
/// Dropping the stream cancels the invocation.
pub struct AgentStream<'a> {
    slot: Slot,
    future: Option<LoopFuture<'a>>,
}

impl<'a> AgentStream<'a> {
    pub(crate) fn new<F>(printer: Option<Arc<Mutex<AgentPrinter>>>, run: impl FnOnce(Emitter) -> F) -> Self
    where
        F: Future<Output = Result<AgentResult>> + Send + 'a,
    {
        let slot: Slot = Arc::new(Mutex::new(None));
        let emitter = Emitter {
            slot: Arc::clone(&slot),
            printer,
        };
        Self {
            slot,
            future: Some(Box::pin(run(emitter))),
        }
    }

    fn take_event(&self) -> Option<AgentStreamEvent> {
        self.slot.lock().unwrap_or_else(PoisonError::into_inner).take()
    }
}

impl Stream for AgentStream<'_> {
    type Item = Result<AgentStreamEvent>;

    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        if let Some(event) = self.take_event() {
            return Poll::Ready(Some(Ok(event)));
        }
        let Some(future) = self.future.as_mut() else {
            return Poll::Ready(None);
        };
        let polled = future.as_mut().poll(cx);
        if let Some(event) = self.take_event() {
            return Poll::Ready(Some(Ok(event)));
        }
        match polled {
            Poll::Pending => Poll::Pending,
            Poll::Ready(result) => {
                self.future = None;
                Poll::Ready(Some(result.map(AgentStreamEvent::AgentResult)))
            }
        }
    }
}

impl fmt::Debug for AgentStream<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AgentStream")
            .field("finished", &self.future.is_none())
            .finish_non_exhaustive()
    }
}
