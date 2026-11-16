//! Hooks (TS `src/hooks/`): typed callbacks on agent lifecycle events.

pub mod events;
pub mod registry;
pub mod types;

pub use events::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, Cancel,
    ContentBlockEvent, EndTurn, InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent,
    ModelStopData, ModelStreamUpdateEvent, Redaction, ToolResultEvent, ToolStreamUpdateEvent,
};
pub use registry::{EventKind, Hook, HookEvent, HookRegistry, IntoSyncHook};
pub use types::{
    HookCallbackOptions, HookCallbackOptionsBuilder, HookCleanup, HookOrder, hook_callback_options_builder,
};
