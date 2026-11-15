//! Hooks: typed callbacks the agent runs at each step of an invocation (TS `hooks/`).
//!
//! Register a sync closure with [`HookRegistry::add_hook`] (the argument type picks the event),
//! an async one with [`HookRegistry::add_async_hook`], or a [`Hook`] implementation with
//! [`HookRegistry::add_hook_impl`]. Each returns a [`HookCleanup`] that removes the hook.

pub mod events;
#[cfg(test)]
pub(crate) mod fixtures;
pub mod registry;
pub mod types;

pub use events::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, Cancel,
    ContentBlockEvent, EndTurn, InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent,
    ModelStopData, ModelStreamUpdateEvent, ToolCallError, ToolResultEvent, ToolStreamUpdateEvent,
};
pub use registry::{EventKind, HookEvent, HookRegistry, IntoHookImpl, IntoSyncHook, Slot};
pub use types::{Hook, HookCleanup, HookOptions, HookOrder};
