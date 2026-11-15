//! The built-in model plugin (TS `plugins/model-plugin.ts`).

use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::hooks::AfterInvocationEvent;
use crate::models::model::Model;
use crate::plugins::Plugin;
use crate::types::agent::LocalAgent;

/// Clears the history after each invocation when the model keeps the conversation server-side
/// (TS `ModelPlugin`, named `strands:model`). Registered last, so its `AfterInvocationEvent`
/// hook runs first.
pub(crate) struct ModelLifecycle {
    model: Arc<dyn Model>,
}

impl ModelLifecycle {
    pub(crate) fn new(model: Arc<dyn Model>) -> Self {
        Self { model }
    }
}

#[async_trait]
impl Plugin for ModelLifecycle {
    fn name(&self) -> &str {
        "strands:model"
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        let model = Arc::clone(&self.model);
        agent
            .hooks()
            .add_hook(move |event: &mut AfterInvocationEvent<'_>| {
                if model.stateful() {
                    event.agent_mut().messages_mut().clear();
                    tracing::debug!("cleared messages for server-managed conversation");
                }
            });
        Ok(())
    }
}
