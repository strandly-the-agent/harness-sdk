//! Test helpers for tools (TS `__fixtures__/tool-helpers.ts`).

use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;

use crate::agent::invocation_state::InvocationState;
use crate::agent::tool_agent::{ToolAgent, ToolAgentParts, ToolCallOutcome, ToolCaller};
use crate::cancel::CancelSignal;
use crate::models::model::Model;
use crate::state_store::StateStore;
use crate::testing::MockModel;
use crate::tools::context::ToolContext;
use crate::tools::registry::ToolRegistry;
use crate::types::json::JsonValue;
use crate::types::messages::ToolResultBlock;
use crate::types::tools::{ToolResultStatus, ToolUse};

/// A [`ToolCaller`] double that records calls and answers with a scripted outcome.
pub(crate) struct RecordingCaller {
    pub(crate) calls: Mutex<Vec<(String, JsonValue)>>,
    pub(crate) outcome: ToolCallOutcome,
}

impl RecordingCaller {
    pub(crate) fn new(outcome: ToolCallOutcome) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            outcome,
        }
    }

    pub(crate) fn calls(&self) -> Vec<(String, JsonValue)> {
        self.calls.lock().unwrap_or_else(PoisonError::into_inner).clone()
    }
}

#[async_trait]
impl ToolCaller for RecordingCaller {
    async fn call_tool(&self, _agent: &ToolAgent, name: &str, input: JsonValue) -> ToolCallOutcome {
        self.calls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((name.to_owned(), input));
        self.outcome.clone()
    }
}

/// The parts of a mock agent (TS `createMockContext`'s `mock-agent`).
pub(crate) fn mock_agent_parts(tool_caller: Arc<dyn ToolCaller>) -> ToolAgentParts {
    let model: Arc<dyn Model> = Arc::new(MockModel::new());
    let (_signal, cancel_handle) = CancelSignal::new();
    ToolAgentParts {
        id: "mock-agent".to_owned(),
        name: "Strands Agent".to_owned(),
        app_state: StateStore::new(),
        aux_model: Arc::clone(&model),
        model,
        cancel_handle,
        messages: Arc::from(Vec::new()),
        tool_registry: Arc::new(ToolRegistry::new()),
        invocation_context: None,
        tool_caller,
    }
}

pub(crate) fn mock_agent() -> ToolAgent {
    let caller = RecordingCaller::new(ToolCallOutcome::Result(ToolResultBlock::new(
        "",
        ToolResultStatus::Success,
        Vec::new(),
    )));
    ToolAgent::new(mock_agent_parts(Arc::new(caller)))
}

/// A context for calling a tool with `tool_use` outside an agent (TS `createMockContext`).
pub(crate) fn mock_context(tool_use: ToolUse) -> ToolContext {
    let (signal, _handle) = CancelSignal::new();
    ToolContext::new(tool_use, mock_agent(), InvocationState::new(), signal)
}
