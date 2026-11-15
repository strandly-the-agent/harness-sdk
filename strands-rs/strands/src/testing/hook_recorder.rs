//! A hook callback recorder for tests.

use std::sync::{Arc, Mutex, PoisonError};

use crate::hooks::{
    AfterInvocationEvent, AfterModelCallEvent, AfterToolCallEvent, AfterToolsEvent, AgentResultEvent,
    BeforeInvocationEvent, BeforeModelCallEvent, BeforeToolCallEvent, BeforeToolsEvent, ContentBlockEvent,
    HookRegistry, InitializedEvent, InterruptEvent, MessageAddedEvent, ModelMessageEvent,
    ModelStreamUpdateEvent, ToolResultEvent, ToolStreamUpdateEvent,
};

/// Records the order hook events fire in, by their TS `type` names (`beforeInvocationEvent`,
/// ...), the role the TS `MockPlugin` plays in the TS tests.
///
/// ```
/// use strands::hooks::HookRegistry;
/// use strands::testing::HookRecorder;
///
/// let registry = HookRegistry::new();
/// let recorder = HookRecorder::new();
/// recorder.register(&registry);
/// assert!(recorder.events().is_empty());
/// ```
#[derive(Debug, Clone, Default)]
pub struct HookRecorder {
    events: Arc<Mutex<Vec<&'static str>>>,
}

macro_rules! record {
    ($self:ident, $registry:ident, $($event:ty => $name:literal),* $(,)?) => {
        $(
            let events = Arc::clone(&$self.events);
            $registry.add_hook(move |_: &mut $event| {
                events.lock().unwrap_or_else(PoisonError::into_inner).push($name);
            });
        )*
    };
}

impl HookRecorder {
    /// An empty recorder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers a recording callback for every hook event on `registry`.
    pub fn register(&self, registry: &HookRegistry) {
        record!(self, registry,
            InitializedEvent<'_> => "initializedEvent",
            BeforeInvocationEvent<'_> => "beforeInvocationEvent",
            AfterInvocationEvent<'_> => "afterInvocationEvent",
            MessageAddedEvent<'_> => "messageAddedEvent",
            BeforeToolCallEvent => "beforeToolCallEvent",
            AfterToolCallEvent => "afterToolCallEvent",
            BeforeModelCallEvent<'_> => "beforeModelCallEvent",
            AfterModelCallEvent<'_> => "afterModelCallEvent",
            ModelStreamUpdateEvent<'_> => "modelStreamUpdateEvent",
            ContentBlockEvent<'_> => "contentBlockEvent",
            ModelMessageEvent<'_> => "modelMessageEvent",
            ToolResultEvent => "toolResultEvent",
            ToolStreamUpdateEvent => "toolStreamUpdateEvent",
            AgentResultEvent<'_> => "agentResultEvent",
            InterruptEvent<'_> => "interruptEvent",
            BeforeToolsEvent<'_> => "beforeToolsEvent",
            AfterToolsEvent<'_> => "afterToolsEvent",
        );
    }

    /// The recorded event names, in firing order.
    pub fn events(&self) -> Vec<&'static str> {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }

    /// Forgets the recorded events.
    pub fn clear(&self) {
        self.events.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }
}
