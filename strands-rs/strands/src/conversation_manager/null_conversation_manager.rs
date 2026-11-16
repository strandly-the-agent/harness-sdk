//! The no-op manager (TS `conversation-manager/null-conversation-manager.ts`).

use async_trait::async_trait;

use crate::conversation_manager::{ConversationManager, ConversationManagerReduceOptions};
use crate::error::Result;

/// Never changes the history (TS `NullConversationManager`): an overflow isn't retried, and
/// there is no proactive compression.
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct NullConversationManager;

impl NullConversationManager {
    /// The manager name (TS `name`).
    pub const NAME: &'static str = "strands:null-conversation-manager";

    /// The manager.
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl ConversationManager for NullConversationManager {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn reduce(&self, _options: ConversationManagerReduceOptions<'_>) -> Result<bool> {
        Ok(false)
    }
}
