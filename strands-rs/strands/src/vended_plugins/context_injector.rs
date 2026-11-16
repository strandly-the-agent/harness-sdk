//! The context-injection plugin (TS `vended-plugins/context-injector/`): writes just-in-time text
//! into the latest user message before each model call, as an [`InjectedBlock`] that stays in
//! the conversation.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use async_trait::async_trait;

use crate::error::Result;
use crate::injection::{InjectionCondition, RenderContent, render_content};
pub use crate::injection::{InjectionContext, InjectionTrigger};
use crate::middleware::{InvokeModelContext, InvokeModelStage, MiddlewareStage};
use crate::plugins::Plugin;
use crate::types::agent::LocalAgent;
use crate::types::messages::{ContentBlock, InjectedBlock, Message, Role};

/// Configuration of [`ContextInjector`] (TS `ContextInjectorConfig`).
///
/// `render_content` is required, so there is no `Default`: start from the builder, or from
/// [`ContextInjectorConfig::new`] in a struct literal.
///
/// ```
/// use strands::injection::InjectionTrigger;
/// use strands::vended_plugins::context_injector::ContextInjectorConfig;
///
/// let config = ContextInjectorConfig::builder()
///     .name("my:clock")
///     .trigger(InjectionTrigger::EveryTurn)
///     .render_content(|_context| async { Ok(Some("<now>12:00</now>".to_owned())) })
///     .build();
/// assert_eq!(config.name.as_deref(), Some("my:clock"));
///
/// let config = ContextInjectorConfig {
///     name: Some("my:cwd".into()),
///     ..ContextInjectorConfig::new(|_context| async { Ok(Some("<cwd>/repo</cwd>".to_owned())) })
/// };
/// assert!(config.trigger.is_none());
/// ```
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Clone, bon::Builder)]
#[builder(on(String, into), state_mod(vis = "pub"))]
pub struct ContextInjectorConfig {
    /// The plugin name (default `strands:context-injector`). It is stored with every block the
    /// injector writes, so keep it stable and give each injector its own.
    pub name: Option<String>,
    /// When to inject (default [`InjectionTrigger::UserTurn`]): a trigger, or
    /// [`InjectionCondition::predicate`] for a function of the call.
    #[builder(into)]
    pub trigger: Option<InjectionCondition>,
    /// Returns the text to inject; `None` or blank text skips the call, and an error fails open
    /// (the model call proceeds without it). Box a closure with
    /// [`render_content`].
    ///
    /// The text reaches the model verbatim, so escape any attacker-influenced fields. It is
    /// rendered at most once per user message: a retry or a resume sends the stored text.
    #[builder(setters(vis = "", name = render_content_boxed))]
    pub render_content: RenderContent,
}

opaque_builder_debug!(ContextInjectorConfigBuilder, context_injector_config_builder);

impl<S: context_injector_config_builder::State> ContextInjectorConfigBuilder<S> {
    /// Sets the function that returns the text to inject; see
    /// [`ContextInjectorConfig::render_content`].
    pub fn render_content<F, Fut>(
        self,
        render: F,
    ) -> ContextInjectorConfigBuilder<context_injector_config_builder::SetRenderContent<S>>
    where
        S::RenderContent: context_injector_config_builder::IsUnset,
        F: Fn(InjectionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<String>>> + Send + 'static,
    {
        self.render_content_boxed(render_content(render))
    }
}

impl ContextInjectorConfig {
    /// A configuration with the default name and trigger that injects the text `render` returns.
    pub fn new<F, Fut>(render: F) -> Self
    where
        F: Fn(InjectionContext) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<String>>> + Send + 'static,
    {
        Self {
            name: None,
            trigger: None,
            render_content: render_content(render),
        }
    }
}

impl fmt::Debug for ContextInjectorConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextInjectorConfig")
            .field("name", &self.name)
            .field("trigger", &self.trigger)
            .finish_non_exhaustive()
    }
}

/// A plugin that injects just-in-time context before each model call (TS `ContextInjector`).
///
/// The rendered text is written into the latest user message as an [`InjectedBlock`], which
/// stays in the conversation and the session. A model that takes temporary messages receives
/// each block itself; every other model receives only the latest user message's blocks, as text.
/// Several injectors may be registered; each writes its own block, in registration order.
///
/// ```
/// use strands::testing::MockMessageModel;
/// use strands::vended_plugins::context_injector::{ContextInjector, ContextInjectorConfig};
/// use strands::Agent;
///
/// let injector = ContextInjector::new(ContextInjectorConfig::new(|_context| async {
///     Ok(Some("<cwd>/repo</cwd>".to_owned()))
/// }));
/// let agent = Agent::builder().model(MockMessageModel::new()).plugin(injector).build()?;
/// # Ok::<(), strands::Error>(())
/// ```
#[derive(Clone)]
pub struct ContextInjector {
    name: String,
    trigger: InjectionCondition,
    render_content: RenderContent,
}

impl fmt::Debug for ContextInjector {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ContextInjector")
            .field("name", &self.name)
            .field("trigger", &self.trigger)
            .finish_non_exhaustive()
    }
}

impl ContextInjector {
    /// The plugin name of an injector whose config sets none (TS `DEFAULT_NAME`). It isn't
    /// `NAME` as on other plugins because the config can replace it.
    pub const DEFAULT_NAME: &'static str = "strands:context-injector";

    /// An injector with `config`.
    pub fn new(config: ContextInjectorConfig) -> Self {
        Self {
            name: config.name.unwrap_or_else(|| Self::DEFAULT_NAME.to_owned()),
            trigger: config.trigger.unwrap_or_default(),
            render_content: config.render_content,
        }
    }

    /// Writes the rendered text into the latest user message of the conversation and of this
    /// call. A message gets at most one block from this injector, so a retry or a resume sends
    /// the same request again. Anything that skips leaves the conversation unchanged.
    async fn inject(&self, mut context: InvokeModelContext) -> InvokeModelContext {
        let mut history = context.agent().current_messages();
        let Some(latest) = history.last().filter(|latest| latest.role() == Role::User) else {
            return context;
        };
        if latest.content().iter().any(|block| self.wrote(block)) {
            return context;
        }
        // Another middleware may have replaced this call's messages.
        let Some(index) = context
            .messages
            .iter()
            .position(|message| message.tracking_id() == latest.tracking_id())
        else {
            return context;
        };
        let injection_context = InjectionContext::new(context.messages.clone(), context.agent().clone());
        if !self.trigger.fires(&injection_context) {
            return context;
        }
        let text = match (self.render_content)(injection_context).await {
            Ok(text) => text,
            Err(error) => {
                tracing::warn!("reason=<{error}> | injection renderContent threw; skipping injection");
                return context;
            }
        };
        let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
            return context;
        };
        let block = InjectedBlock::new(text, self.name.clone());
        // After the stored content, so text other middleware adds for this call only stays last.
        let position = latest.content().len();
        let written = with_block(latest, &block, position);
        if let Some(last) = history.last_mut() {
            *last = written;
        }
        context.agent().replace_messages(history);
        if let Some(message) = context.messages.get_mut(index) {
            *message = with_block(message, &block, position);
        }
        context
    }

    fn wrote(&self, block: &ContentBlock) -> bool {
        matches!(block, ContentBlock::Injected(injected) if injected.source() == self.name)
    }
}

fn with_block(message: &Message, block: &InjectedBlock, position: usize) -> Message {
    let mut content = message.content().to_vec();
    content.insert(position.min(content.len()), block.clone().into());
    let mut written = Message::new(message.role(), content).with_tracking_id(message.tracking_id());
    if let Some(metadata) = message.metadata() {
        written = written.with_metadata(metadata.clone());
    }
    written
}

#[async_trait]
impl Plugin for ContextInjector {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init_agent(self: Arc<Self>, agent: &mut LocalAgent) -> Result<()> {
        let _cleanup = agent.add_middleware(InvokeModelStage::input(move |context| {
            let injector = Arc::clone(&self);
            async move { Ok(injector.inject(context).await) }
        }));
        Ok(())
    }
}

#[cfg(test)]
mod tests;
