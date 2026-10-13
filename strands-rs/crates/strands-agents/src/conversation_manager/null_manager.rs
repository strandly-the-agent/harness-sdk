//! No-op conversation manager (`strands-ts/src/conversation-manager/null-conversation-manager.ts`).

use async_trait::async_trait;

use super::{ConversationManager, ReduceOptions};
use crate::agent::Agent;
use crate::error::Result;
use crate::plugins::Plugin;

/// A conversation manager that never modifies the conversation history.
///
/// The base hooks are still registered, but because `reduce` always returns `false` overflow
/// errors are never retried.
#[derive(Clone, Copy, Debug, Default)]
pub struct NullConversationManager;

impl NullConversationManager {
    /// New no-op manager.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Plugin for NullConversationManager {
    fn name(&self) -> &str {
        "strands:null-conversation-manager"
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        self.register_base_hooks(agent);
        Ok(())
    }
}

#[async_trait]
impl ConversationManager for NullConversationManager {
    async fn reduce(&self, _options: ReduceOptions<'_>) -> Result<bool> {
        Ok(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::conversation_manager::test_support::test_agent_with_messages;
    use crate::conversation_manager::test_support::{after_model_call, overflow};
    use crate::hooks::{AfterModelCallEvent, BeforeModelCallEvent};
    use crate::types::{ContentBlock, Message};

    #[tokio::test]
    async fn does_not_modify_conversation_history_on_overflow() {
        let agent = test_agent_with_messages(vec![Message::user("Hello"), Message::assistant("Hi there")]);
        NullConversationManager::new().init_agent(&agent).await.unwrap();

        let mut event = after_model_call(&agent, Some(overflow()));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        let messages = agent.messages();
        assert_eq!(messages.len(), 2);
        assert_eq!(messages[0].content[0], ContentBlock::text("Hello"));
        assert_eq!(messages[1].content[0], ContentBlock::text("Hi there"));
    }

    #[tokio::test]
    async fn does_not_set_retry_on_context_overflow() {
        let agent = test_agent_with_messages(vec![]);
        NullConversationManager::new().init_agent(&agent).await.unwrap();

        let mut event = after_model_call(&agent, Some(overflow()));
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();

        assert!(!event.retry);
    }

    #[tokio::test]
    async fn registers_both_the_overflow_recovery_and_proactive_compression_hooks() {
        let agent = test_agent_with_messages(vec![]);
        NullConversationManager::new().init_agent(&agent).await.unwrap();

        assert_eq!(agent.hooks().count::<AfterModelCallEvent>(), 1);
        assert_eq!(agent.hooks().count::<BeforeModelCallEvent>(), 1);
    }

    #[test]
    fn returns_the_plugin_name() {
        assert_eq!(
            NullConversationManager::new().name(),
            "strands:null-conversation-manager"
        );
    }
}
