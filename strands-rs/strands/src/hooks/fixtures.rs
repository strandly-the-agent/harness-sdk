//! Test helpers for hooks: a local agent and events to fire.

use std::sync::{Arc, Mutex, PoisonError};

use crate::agent::invocation_state::InvocationState;
use crate::agent::local_agent::{LocalAgent, LocalAgentParts};
use crate::hooks::events::BeforeToolCallEvent;
use crate::hooks::registry::HookRegistry;
use crate::models::model::Model;
use crate::state_store::StateStore;
use crate::testing::MockModel;
use crate::tools::fixtures::mock_agent;
use crate::tools::registry::ToolRegistry;
use crate::types::tools::ToolUse;

/// A local agent with a mock model and its own registry (TS `new Agent()` in the hook tests).
pub(crate) fn mock_local_agent() -> LocalAgent {
    let model: Arc<dyn Model> = Arc::new(MockModel::new());
    LocalAgent::new(LocalAgentParts {
        id: "agent".to_owned(),
        name: "Strands Agent".to_owned(),
        description: None,
        messages: Vec::new(),
        system_prompt: None,
        model: Some(model),
        aux_model: None,
        tool_registry: ToolRegistry::new(),
        app_state: StateStore::new(),
        model_state: StateStore::new(),
        hooks: Arc::new(HookRegistry::new()),
    })
}

/// A `BeforeToolCallEvent` for tool `test` with id `tool-1` and no registry tool.
pub(crate) fn before_tool_call() -> BeforeToolCallEvent {
    BeforeToolCallEvent::new(
        mock_agent(),
        ToolUse::new("test", "tool-1", serde_json::json!({})),
        None,
        InvocationState::new(),
    )
}

/// Records what hooks did, in order.
#[derive(Clone, Default)]
pub(crate) struct Calls(Arc<Mutex<Vec<String>>>);

impl Calls {
    pub(crate) fn push(&self, call: impl Into<String>) {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(call.into());
    }

    pub(crate) fn get(&self) -> Vec<String> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}
