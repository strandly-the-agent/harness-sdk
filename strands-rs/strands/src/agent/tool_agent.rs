//! The agent view per-tool hooks and tools see (design: `ToolAgent`).

use std::fmt;
use std::sync::Arc;

use async_trait::async_trait;

use crate::agent::invocation_context::InvocationContext;
use crate::cancel::CancelHandle;
use crate::interrupt::Interrupt;
use crate::models::model::Model;
use crate::state_store::StateStore;
use crate::tools::registry::ToolRegistry;
use crate::types::json::JsonValue;
use crate::types::messages::{Message, ToolResultBlock};

/// Runs a registry tool through the full per-tool pipeline for [`ToolAgent::call_tool`].
///
/// The agent loop installs the implementation: hooks, interventions, middleware and the tool.
#[async_trait]
pub(crate) trait ToolCaller: Send + Sync + 'static {
    async fn call_tool(&self, agent: &ToolAgent, name: &str, input: JsonValue) -> ToolCallOutcome;
}

/// What [`ToolAgent::new`] is built from.
pub(crate) struct ToolAgentParts {
    pub(crate) id: String,
    pub(crate) name: String,
    pub(crate) app_state: StateStore,
    pub(crate) model: Arc<dyn Model>,
    pub(crate) aux_model: Arc<dyn Model>,
    pub(crate) cancel_handle: CancelHandle,
    pub(crate) messages: Arc<[Message]>,
    pub(crate) tool_registry: Arc<ToolRegistry>,
    pub(crate) invocation_context: Option<InvocationContext>,
    pub(crate) tool_caller: Arc<dyn ToolCaller>,
}

/// The agent as per-tool hooks and tools see it: a `'static`, cheaply cloned view built for each
/// tool batch.
///
/// The messages are a snapshot taken when the batch started, and the registry is read-only, so
/// tools can't rewrite history mid-batch. App state, models and cancellation are the agent's live
/// shared handles.
#[non_exhaustive]
#[derive(Clone)]
pub struct ToolAgent {
    inner: Arc<ToolAgentParts>,
}

impl ToolAgent {
    #[cfg_attr(not(test), expect(dead_code, reason = "built by the agent loop"))]
    pub(crate) fn new(parts: ToolAgentParts) -> Self {
        Self {
            inner: Arc::new(parts),
        }
    }

    /// The agent's id.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.inner.id
    }

    /// The agent's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.inner.name
    }

    /// The agent's app state (TS `appState`), shared with the agent.
    #[must_use]
    pub fn app_state(&self) -> &StateStore {
        &self.inner.app_state
    }

    /// The agent's model.
    #[must_use]
    pub fn model(&self) -> &Arc<dyn Model> {
        &self.inner.model
    }

    /// The model for auxiliary calls (TS `auxModel`); the agent's model unless one was set.
    #[must_use]
    pub fn aux_model(&self) -> &Arc<dyn Model> {
        &self.inner.aux_model
    }

    /// A handle that cancels the running invocation.
    #[must_use]
    pub fn cancel_handle(&self) -> &CancelHandle {
        &self.inner.cancel_handle
    }

    /// The conversation as it was when the tool batch started.
    #[must_use]
    pub fn messages(&self) -> &Arc<[Message]> {
        &self.inner.messages
    }

    /// The agent's tools, read-only.
    #[must_use]
    pub fn tool_registry(&self) -> &ToolRegistry {
        &self.inner.tool_registry
    }

    /// The request the running invocation belongs to.
    #[must_use]
    pub fn invocation_context(&self) -> Option<&InvocationContext> {
        self.inner.invocation_context.as_ref()
    }

    /// Calls the registry tool `name` with `input` through the full per-tool pipeline: hooks,
    /// interventions, middleware and the tool itself.
    ///
    /// An interrupt or a cancel comes back as its own outcome and is never recorded in the
    /// agent's interrupt state; the caller decides what to do with it.
    pub async fn call_tool(&self, name: &str, input: JsonValue) -> ToolCallOutcome {
        self.inner.tool_caller.call_tool(self, name, input).await
    }
}

impl fmt::Debug for ToolAgent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ToolAgent")
            .field("id", &self.inner.id)
            .field("name", &self.inner.name)
            .finish_non_exhaustive()
    }
}

/// How a [`ToolAgent::call_tool`] call ended.
#[non_exhaustive]
#[derive(Debug, Clone, PartialEq)]
pub enum ToolCallOutcome {
    /// The tool ran (or failed) and produced this result.
    Result(ToolResultBlock),
    /// A hook or the tool raised these interrupts, for example an approval request.
    Interrupt(Vec<Interrupt>),
    /// A hook or the cancel signal cancelled the call; the result holds the cancel message.
    Cancelled(ToolResultBlock),
}

#[cfg(test)]
mod tests;
