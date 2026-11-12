//! Hooks: callbacks on agent lifecycle events (TS `hooks/`).
//!
//! Register a hook on an agent with `add_hook` (a synchronous closure whose argument type names
//! the event), `add_async_hook::<E>` (a closure returning a boxed future) or `add_hook_impl` (a
//! [`Hook`] implementation). Each returns a [`HookCleanup`] that removes it.

mod events;
mod registry;
mod types;

pub use events::{
    AfterInvocationEvent, AfterInvocationEventData, AfterModelCallEvent, AfterModelCallEventData,
    AfterToolCallEvent, AfterToolCallEventData, AfterToolsEvent, AfterToolsEventData, AgentResultEvent,
    BeforeInvocationEvent, BeforeInvocationEventData, BeforeModelCallEvent, BeforeModelCallEventData,
    BeforeToolCallEvent, BeforeToolCallEventData, BeforeToolsEvent, BeforeToolsEventData, Cancel,
    ContentBlockEvent, ContentBlockEventData, EndTurn, EventErrorData, InitializedEvent, InterruptEvent,
    InterruptEventData, MessageAddedEvent, MessageAddedEventData, ModelMessageEvent, ModelMessageEventData,
    ModelStopData, ModelStreamUpdateEvent, ModelStreamUpdateEventData, ToolResultEvent, ToolResultEventData,
    ToolStreamUpdateEvent, ToolStreamUpdateEventData,
};
pub use registry::{BoxFuture, EventKind, Hook, HookEvent, HookRegistry, IntoSyncHook, kinds};
#[doc(hidden)]
pub use registry::{HookSlot, Slots};
pub use types::{HookCleanup, HookOptions, HookOrder};

pub use crate::models::Redaction;
