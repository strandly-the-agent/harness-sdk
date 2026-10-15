//! The `InvokableAgent` trait: the invocable surface shared by a local [`Agent`] and a remote
//! one (`InvokableAgent` in `strands-ts/src/types/agent.ts`).

use async_trait::async_trait;

use crate::agent::{Agent, AgentResult, InvokeInput, InvokeOptions};
use crate::error::Result;

/// Something an orchestrator can invoke like an agent.
///
/// Implemented by [`Agent`] and by [`A2AAgent`](crate::a2a::A2AAgent), so a tool, graph node or
/// delegation target can hold either behind `Arc<dyn InvokableAgent>`. Takes the concrete
/// [`InvokeInput`]/[`InvokeOptions`] rather than `impl Into<_>` so the trait stays object-safe;
/// the generic conveniences live on the concrete types.
#[async_trait]
pub trait InvokableAgent: Send + Sync {
    /// Unique identifier of this agent.
    fn id(&self) -> &str;

    /// The agent's name.
    fn name(&self) -> &str;

    /// What the agent does, when it says.
    fn description(&self) -> Option<&str>;

    /// Invoke the agent and wait for its result.
    async fn invoke_with(&self, input: InvokeInput, options: InvokeOptions) -> Result<AgentResult>;
}

#[async_trait]
impl InvokableAgent for Agent {
    fn id(&self) -> &str {
        Agent::id(self)
    }

    fn name(&self) -> &str {
        Agent::name(self)
    }

    fn description(&self) -> Option<&str> {
        Agent::description(self)
    }

    async fn invoke_with(&self, input: InvokeInput, options: InvokeOptions) -> Result<AgentResult> {
        Agent::invoke_with(self, input, options).await
    }
}

/// Shared agents are invocable too, so `Arc<dyn InvokableAgent>` is itself an `InvokableAgent`.
#[async_trait]
impl<T: InvokableAgent + ?Sized> InvokableAgent for std::sync::Arc<T> {
    fn id(&self) -> &str {
        (**self).id()
    }

    fn name(&self) -> &str {
        (**self).name()
    }

    fn description(&self) -> Option<&str> {
        (**self).description()
    }

    async fn invoke_with(&self, input: InvokeInput, options: InvokeOptions) -> Result<AgentResult> {
        (**self).invoke_with(input, options).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[tokio::test]
    async fn a_local_agent_is_invocable_behind_a_trait_object() {
        let model = crate::testing::MockMessageModel::shared(vec![crate::testing::MockResponse::message(
            crate::types::Message::assistant("hi back"),
        )]);
        let local = Agent::builder().model(model).build().unwrap();
        let agent: Arc<dyn InvokableAgent> = Arc::new(local.clone());
        assert_eq!(agent.id(), local.id());
        assert_eq!(agent.name(), local.name());
        assert_eq!(agent.description(), local.description());

        let result = agent
            .invoke_with(InvokeInput::Text("hi".to_owned()), InvokeOptions::default())
            .await
            .unwrap();
        assert_eq!(result.text(), "hi back");
    }
}
