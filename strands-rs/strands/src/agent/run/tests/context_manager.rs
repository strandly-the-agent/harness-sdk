//! Ported from strands-ts/src/agent/__tests__/agent.context-manager.test.ts: the two cases the
//! context manager's own tests (`experimental::context_manager::tests`) don't assert.

use super::*;
use crate::agent::config::AgentConfig;
use crate::experimental::context_manager::{ContextManager, ContextManagerConfig, ContextManagerPreset};

fn agent_config() -> AgentConfig {
    AgentConfig {
        model: Some(mock(vec![vec![text("hi")]]).into()),
        printer: Some(false),
        ..Default::default()
    }
}

mod when_agentic {
    use super::*;

    #[test]
    fn creates_a_context_manager_instance() {
        let agent = Agent::new(AgentConfig {
            context_manager: Some(ContextManagerPreset::Agentic.into()),
            ..agent_config()
        })
        .unwrap();

        assert!(agent.context_manager().is_some());
    }
}

#[tokio::test]
async fn registers_the_instance_as_a_plugin() {
    let manager = ContextManager::new(ContextManagerConfig::default()).unwrap();
    let mut agent = Agent::new(AgentConfig {
        context_manager: Some(manager.clone().into()),
        ..agent_config()
    })
    .unwrap();

    agent.invoke("hi").await.unwrap();

    let registered: Vec<_> = agent
        .registration_order()
        .into_iter()
        .filter(|plugin| plugin.name() == ContextManager::NAME)
        .collect();
    assert_eq!(registered.len(), 1);
    // The registered plugin is a clone of the handle: initialising it bound the caller's instance.
    assert!(manager.stash().is_some());
}
