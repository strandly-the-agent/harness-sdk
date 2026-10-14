//! `ContextInjector` (`strands-ts/src/vended-plugins/context-injector`): folds ephemeral text
//! into the conversation for a single model call.
//!
//! TS does this with `InvokeModelStage.Input` middleware; Rust uses the equivalent seam,
//! [`BeforeModelCallEvent::messages`] — a per-call copy of the conversation. Durable history and
//! the session are never touched.

use std::sync::Arc;

use async_trait::async_trait;
use futures::future::BoxFuture;

use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::BeforeModelCallEvent;
use crate::plugins::Plugin;
use crate::types::{ContentBlock, Message, Role};

const DEFAULT_NAME: &str = "strands:context-injector";

/// When an injector contributes text.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum InjectionTrigger {
    /// Only on a fresh user ask: the last message is from the user and carries no tool results.
    #[default]
    UserTurn,
    /// Every model call.
    EveryTurn,
}

/// What an injector sees when deciding what to inject.
pub struct InjectionContext {
    /// The conversation as it will be sent to the model.
    pub messages: Vec<Message>,
    /// The invoking agent (its [`state`](Agent::state) is the durable app state).
    pub agent: Agent,
}

type RenderFn =
    Arc<dyn for<'a> Fn(&'a InjectionContext) -> BoxFuture<'a, Result<Option<String>>> + Send + Sync>;
type PredicateFn = Arc<dyn Fn(&InjectionContext) -> bool + Send + Sync>;

#[derive(Clone)]
enum Trigger {
    Builtin(InjectionTrigger),
    Predicate(PredicateFn),
}

/// Appends rendered text to the most recent user message of a single model call.
#[derive(Clone)]
pub struct ContextInjector {
    name: String,
    trigger: Trigger,
    render: RenderFn,
}

impl ContextInjector {
    /// Injector whose text is produced per call by `render`.
    pub fn new<F>(render: F) -> Self
    where
        F: for<'a> Fn(&'a InjectionContext) -> BoxFuture<'a, Result<Option<String>>> + Send + Sync + 'static,
    {
        Self {
            name: DEFAULT_NAME.to_owned(),
            trigger: Trigger::Builtin(InjectionTrigger::UserTurn),
            render: Arc::new(render),
        }
    }

    /// Injector for a fixed string.
    pub fn inject(text: impl Into<String>) -> Self {
        let text = text.into();
        Self::new(move |_ctx| {
            let text = text.clone();
            Box::pin(async move { Ok(Some(text)) })
        })
    }

    /// Override the plugin name (needed when registering more than one injector).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the built-in trigger.
    pub fn with_trigger(mut self, trigger: InjectionTrigger) -> Self {
        self.trigger = Trigger::Builtin(trigger);
        self
    }

    /// Use a custom predicate as the trigger.
    pub fn with_trigger_fn<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&InjectionContext) -> bool + Send + Sync + 'static,
    {
        self.trigger = Trigger::Predicate(Arc::new(predicate));
        self
    }

    fn should_inject(&self, ctx: &InjectionContext) -> bool {
        match &self.trigger {
            Trigger::Builtin(InjectionTrigger::EveryTurn) => true,
            Trigger::Builtin(InjectionTrigger::UserTurn) => is_user_turn(&ctx.messages),
            Trigger::Predicate(predicate) => predicate(ctx),
        }
    }

    /// Render and fold the text into `messages` for this call. Errors from the renderer are
    /// logged and the messages left untouched (fail open).
    pub async fn apply(&self, agent: &Agent, messages: &mut Vec<Message>) {
        let ctx = InjectionContext {
            messages: messages.clone(),
            agent: agent.clone(),
        };
        if !self.should_inject(&ctx) {
            return;
        }
        let text = match (self.render)(&ctx).await {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(error) => {
                tracing::warn!(reason = %error, "injection renderContent threw; skipping injection");
                return;
            }
        };
        if text.trim().is_empty() {
            return;
        }
        fold_into_last_user_message(messages, &text);
    }
}

/// A fresh user ask: the last message is from the user and has no tool-result blocks.
pub fn is_user_turn(messages: &[Message]) -> bool {
    match messages.last() {
        Some(last) => {
            last.role == Role::User
                && !last
                    .content
                    .iter()
                    .any(|b| matches!(b, ContentBlock::ToolResult(_)))
        }
        None => false,
    }
}

/// Append `text` as a text block on the last user message. A `\n\n` separator is inserted when
/// that message already has content and `text` does not start with a newline.
pub fn fold_into_last_user_message(messages: &mut [Message], text: &str) -> bool {
    let Some(target) = messages.iter_mut().rev().find(|m| m.role == Role::User) else {
        return false;
    };
    let separator = if !target.content.is_empty() && !text.starts_with('\n') {
        "\n\n"
    } else {
        ""
    };
    target
        .content
        .push(ContentBlock::text(format!("{separator}{text}")));
    true
}

#[async_trait]
impl Plugin for ContextInjector {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        let injector = self.clone();
        let _ = agent
            .hooks()
            .add_callback::<BeforeModelCallEvent, _>(move |event| {
                let injector = injector.clone();
                Box::pin(async move {
                    let agent = event.agent.clone();
                    injector.apply(&agent, &mut event.messages).await;
                    Ok(())
                })
            });
        Ok(())
    }
}
