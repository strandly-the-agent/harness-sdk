//! Hooks (TS `src/hooks/`): typed callbacks on agent lifecycle events.

#[doc(hidden)]
pub mod events;
#[doc(hidden)]
pub mod registry;
#[doc(hidden)]
pub mod types;

#[doc(inline)]
pub use events::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, Cancel,
    ContentBlockEvent, EndTurn, InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent,
    ModelStopData, ModelStreamUpdateEvent, Redaction, ToolResultEvent, ToolStreamUpdateEvent,
};
#[doc(inline)]
pub use registry::{AsyncHookFn, EventKind, Hook, HookEvent, HookRegistry, IntoSyncHook, SyncHookFn};
#[doc(inline)]
pub use types::{
    HookCallbackOptions, HookCallbackOptionsBuilder, HookCleanup, HookOrder, hook_callback_options_builder,
};
