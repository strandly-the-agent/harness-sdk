//! Minimal [`MultiAgent`] implementation shared by the multi-agent unit tests.

use std::sync::Arc;

use async_trait::async_trait;

use super::events::MultiAgentResultEvent;
use super::multiagent::{MultiAgent, MultiAgentInput, MultiAgentInvokeOptions, MultiAgentStream};
use super::state::MultiAgentResult;
use super::MultiAgentStreamEvent;
use crate::agent::InvocationState;
use crate::error::Result;
use crate::hooks::HookRegistry;

/// Orchestrator that replays canned events and then its canned result.
pub(crate) struct TestOrchestrator {
    id: String,
    hooks: HookRegistry,
    events: Vec<MultiAgentStreamEvent>,
    result: MultiAgentResult,
}

impl TestOrchestrator {
    pub(crate) fn shared(
        id: &str,
        events: Vec<MultiAgentStreamEvent>,
        result: MultiAgentResult,
    ) -> Arc<dyn MultiAgent> {
        Arc::new(Self {
            id: id.to_owned(),
            hooks: HookRegistry::default(),
            events,
            result,
        })
    }
}

#[async_trait]
impl MultiAgent for TestOrchestrator {
    fn id(&self) -> &str {
        &self.id
    }

    fn name(&self) -> &str {
        &self.id
    }

    fn hooks(&self) -> &HookRegistry {
        &self.hooks
    }

    async fn invoke_with(
        &self,
        _input: MultiAgentInput,
        _options: MultiAgentInvokeOptions,
    ) -> Result<MultiAgentResult> {
        Ok(self.result.clone())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }

    fn stream_with(&self, _input: MultiAgentInput, _options: MultiAgentInvokeOptions) -> MultiAgentStream {
        let result = MultiAgentStreamEvent::MultiAgentResult(MultiAgentResultEvent {
            result: self.result.clone(),
            invocation_state: InvocationState::new(),
        });
        let events: Vec<_> = self
            .events
            .iter()
            .cloned()
            .chain(std::iter::once(result))
            .map(Ok)
            .collect();
        MultiAgentStream::new(Box::pin(futures::stream::iter(events)))
    }
}
