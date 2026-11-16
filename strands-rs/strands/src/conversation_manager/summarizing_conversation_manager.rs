//! The summarizing manager (TS `conversation-manager/summarizing-conversation-manager.ts`).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;

use crate::agent::InvocationContext;
use crate::conversation_manager::compression::context_compression::{
    adjust_split_point_for_tool_pairs, generate_summary,
};
use crate::conversation_manager::compression::pin_message::{apply_pin_first, partition_pinned};
use crate::conversation_manager::{
    ConversationManager, ConversationManagerReduceOptions, ProactiveCompression,
};
use crate::error::Result;
use crate::models::model::Model;
use crate::types::agent::LocalAgent;

/// Configuration of [`SummarizingConversationManager`] (TS `SummarizingConversationManagerConfig`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, bon::Builder)]
pub struct SummarizingConversationManagerConfig {
    /// The model that writes summaries; the agent's `aux_model` (by default its model) otherwise.
    pub model: Option<Arc<dyn Model>>,
    /// The share of messages to summarize, clamped to 0.1-0.8; 0.3 by default.
    #[builder(default = 0.3)]
    pub summary_ratio: f64,
    /// How many of the newest messages are never summarized; 10 by default.
    #[builder(default = 10)]
    pub preserve_recent_messages: usize,
    /// The summarizer's system prompt; a built-in prompt asking for a bullet-point summary otherwise.
    #[builder(into)]
    pub summarization_system_prompt: Option<String>,
    /// Compress before a model call whose projected input crosses a threshold.
    #[builder(default, into)]
    pub proactive_compression: ProactiveCompression,
    /// Pin the first this many messages at the first reduction, so they are never summarized.
    pub pin_first: Option<usize>,
}

impl Default for SummarizingConversationManagerConfig {
    fn default() -> Self {
        Self::builder().build()
    }
}

/// Replaces the oldest messages with a model-written summary (TS
/// `SummarizingConversationManager`). It runs on an overflow and, when enabled, proactively;
/// it never splits a tool use from its result and keeps pinned messages.
#[derive(Debug)]
pub struct SummarizingConversationManager {
    model: Option<Arc<dyn Model>>,
    summary_ratio: f64,
    preserve_recent_messages: usize,
    summarization_system_prompt: Option<String>,
    compression_threshold: Option<f64>,
    pin_first: Option<usize>,
    pin_first_applied: AtomicBool,
}

impl Default for SummarizingConversationManager {
    fn default() -> Self {
        Self {
            model: None,
            summary_ratio: 0.3,
            preserve_recent_messages: 10,
            summarization_system_prompt: None,
            compression_threshold: None,
            pin_first: None,
            pin_first_applied: AtomicBool::new(false),
        }
    }
}

impl SummarizingConversationManager {
    /// A manager from `config`.
    ///
    /// # Errors
    ///
    /// The proactive compression threshold is outside (0, 1].
    pub fn new(config: SummarizingConversationManagerConfig) -> Result<Self> {
        Ok(Self {
            model: config.model,
            summary_ratio: config.summary_ratio.clamp(0.1, 0.8),
            preserve_recent_messages: config.preserve_recent_messages,
            summarization_system_prompt: config.summarization_system_prompt,
            compression_threshold: config.proactive_compression.threshold()?,
            pin_first: config.pin_first,
            pin_first_applied: AtomicBool::new(false),
        })
    }

    async fn summarize_oldest(
        &self,
        agent: &mut LocalAgent,
        model: Arc<dyn Model>,
        invocation_context: Option<InvocationContext>,
    ) -> Result<bool> {
        let messages = agent.messages_mut();
        let length = messages.len();
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            clippy::cast_precision_loss,
            reason = "a non-negative share of the message count"
        )]
        let count = ((length as f64 * self.summary_ratio).floor() as usize).max(1);
        let count = count.min(length.saturating_sub(self.preserve_recent_messages));
        if count == 0 {
            tracing::warn!(
                "preserve_recent=<{}>, messages=<{length}> | insufficient messages for summarization",
                self.preserve_recent_messages
            );
            return Ok(false);
        }
        let count = adjust_split_point_for_tool_pairs(messages, count)?;
        if let Some(pin_first) = self.pin_first.filter(|&pin_first| pin_first > 0)
            && !self.pin_first_applied.swap(true, Ordering::Relaxed)
        {
            apply_pin_first(messages, pin_first);
        }
        let (protected, to_summarize) = partition_pinned(messages, 0, count);
        if to_summarize.is_empty() {
            tracing::warn!(
                "messages=<{length}> | all messages in summarize range are protected, unable to reduce"
            );
            return Ok(false);
        }
        let prompt = self.summarization_system_prompt.as_deref();
        let summary = generate_summary(&to_summarize, &model, prompt, invocation_context).await?;
        let messages = agent.messages_mut();
        messages.splice(0..count, protected.into_iter().chain([summary]));
        Ok(true)
    }
}

#[async_trait]
impl ConversationManager for SummarizingConversationManager {
    fn name(&self) -> &str {
        "strands:summarizing-conversation-manager"
    }

    fn compression_threshold(&self) -> Option<f64> {
        self.compression_threshold
    }

    async fn reduce(&self, mut options: ConversationManagerReduceOptions<'_>) -> Result<bool> {
        let overflow = options.error().is_some();
        let model = self
            .model
            .clone()
            .unwrap_or_else(|| Arc::clone(options.agent().aux_model()));
        let context = options.invocation_context().cloned();
        match self.summarize_oldest(options.agent_mut(), model, context).await {
            Ok(reduced) => Ok(reduced),
            Err(error) if overflow => {
                tracing::error!("error=<{error}> | summarization failed");
                Err(error)
            }
            Err(error) => {
                tracing::warn!("error=<{error}> | proactive summarization failed, continuing");
                Ok(false)
            }
        }
    }
}

#[cfg(test)]
mod tests;
