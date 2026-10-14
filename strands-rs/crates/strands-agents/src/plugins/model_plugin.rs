//! The built-in model plugin (`strands-ts/src/plugins/model-plugin.ts`).

use std::sync::Arc;

use async_trait::async_trait;

use super::Plugin;
use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::AfterInvocationEvent;
use crate::models::Model;

/// Built-in plugin managing model-related lifecycle hooks.
///
/// When the model is stateful (server-managed conversation state), it clears the agent's local
/// message history after each invocation: the server holds the authoritative conversation.
///
/// Internal: wired up automatically by the agent, not meant to be constructed by consumers.
pub struct ModelPlugin {
    model: Arc<dyn Model>,
}

impl ModelPlugin {
    /// Plugin for `model`.
    pub fn new(model: Arc<dyn Model>) -> Self {
        Self { model }
    }
}

#[async_trait]
impl Plugin for ModelPlugin {
    fn name(&self) -> &str {
        "strands:model"
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        let model = Arc::clone(&self.model);
        // Clears `event.agent` rather than a captured handle: capturing the agent here would make
        // the agent own a plugin that owns the agent.
        let _ = agent
            .hooks()
            .add_sync_callback::<AfterInvocationEvent, _>(move |event| {
                if model.stateful() {
                    event.agent.with_messages(Vec::clear);
                    tracing::debug!("cleared messages for server-managed conversation");
                }
                Ok(())
            });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::{test_agent_for_plugins, test_agent_for_plugins_stateful, InvocationState};
    use crate::hooks::AfterInvocationEvent;
    use crate::types::Message;

    fn after_invocation(agent: &Agent) -> AfterInvocationEvent {
        AfterInvocationEvent {
            agent: agent.clone(),
            invocation_state: InvocationState::new(),
            resume: None,
        }
    }

    #[tokio::test]
    async fn exposes_the_plugin_name() {
        let agent = test_agent_for_plugins();
        assert_eq!(ModelPlugin::new(agent.model()).name(), "strands:model");
    }

    #[tokio::test]
    async fn registers_an_after_invocation_hook() {
        let agent = test_agent_for_plugins();
        ModelPlugin::new(agent.model()).init_agent(&agent).await.unwrap();
        assert_eq!(agent.hooks().count::<AfterInvocationEvent>(), 1);
    }

    #[tokio::test]
    async fn clears_messages_after_invocation_when_the_model_is_stateful() {
        let agent = test_agent_for_plugins_stateful();
        agent.set_messages(vec![Message::user("hi")]);
        ModelPlugin::new(agent.model()).init_agent(&agent).await.unwrap();

        let mut event = after_invocation(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert!(agent.messages().is_empty());
    }

    #[tokio::test]
    async fn keeps_messages_after_invocation_when_the_model_is_stateless() {
        let agent = test_agent_for_plugins();
        agent.set_messages(vec![Message::user("hi")]);
        ModelPlugin::new(agent.model()).init_agent(&agent).await.unwrap();

        let mut event = after_invocation(&agent);
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert_eq!(agent.messages().len(), 1);
    }
}
