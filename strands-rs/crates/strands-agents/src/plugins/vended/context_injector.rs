//! `ContextInjector` (`strands-ts/src/vended-plugins/context-injector`): folds ephemeral text
//! into the conversation for a single model call.
//!
//! The plugin is a thin wrapper over [`MessageInjection`](crate::injection::MessageInjection);
//! the delivery primitives live in [`crate::injection`]. TS does this with
//! `InvokeModelStage.Input` middleware; Rust uses the equivalent seam, [`BeforeModelCallEvent`] —
//! a per-call copy of the conversation. Durable history and the session are never touched.

use async_trait::async_trait;
use futures::future::BoxFuture;

use crate::agent::Agent;
use crate::error::Result;
use crate::hooks::BeforeModelCallEvent;
use crate::injection::{FoldOutcome, MessageInjection};
use crate::plugins::Plugin;
use crate::types::Message;

pub use crate::injection::{fold_into_last_user_message, is_user_turn, InjectionContext, InjectionTrigger};

const DEFAULT_NAME: &str = "strands:context-injector";

/// Appends rendered text to the most recent user message of a single model call.
#[derive(Clone)]
pub struct ContextInjector {
    name: String,
    injection: MessageInjection,
}

impl ContextInjector {
    /// Injector whose text is produced per call by `render`.
    pub fn new<F>(render: F) -> Self
    where
        F: for<'a> Fn(&'a InjectionContext) -> BoxFuture<'a, Result<Option<String>>> + Send + Sync + 'static,
    {
        Self {
            name: DEFAULT_NAME.to_owned(),
            injection: MessageInjection::new(render),
        }
    }

    /// Injector for a fixed string.
    pub fn inject(text: impl Into<String>) -> Self {
        Self {
            name: DEFAULT_NAME.to_owned(),
            injection: MessageInjection::text(text),
        }
    }

    /// Override the plugin name (needed when registering more than one injector).
    pub fn with_name(mut self, name: impl Into<String>) -> Self {
        self.name = name.into();
        self
    }

    /// Set the built-in trigger.
    pub fn with_trigger(mut self, trigger: InjectionTrigger) -> Self {
        self.injection = self.injection.with_trigger(trigger);
        self
    }

    /// Use a custom predicate as the trigger.
    pub fn with_trigger_fn<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&InjectionContext) -> bool + Send + Sync + 'static,
    {
        self.injection = self.injection.with_trigger_fn(predicate);
        self
    }

    /// Render and fold the text into `messages` for this call. Errors from the renderer are
    /// logged and the messages left untouched (fail open).
    pub async fn apply(&self, agent: &Agent, messages: &mut [Message]) -> FoldOutcome {
        self.injection.apply(agent, messages).await
    }
}

#[async_trait]
impl Plugin for ContextInjector {
    fn name(&self) -> &str {
        &self.name
    }

    async fn init_agent(&self, agent: &Agent) -> Result<()> {
        let injector = self.clone();
        agent
            .hooks()
            .add_callback_boxed::<BeforeModelCallEvent, _>(move |event| {
                let injector = injector.clone();
                Box::pin(async move {
                    let agent = event.agent.clone();
                    let outcome = injector.apply(&agent, &mut event.messages).await;
                    event.dynamic_trailing_blocks += outcome.trailing_blocks;
                    Ok(())
                })
            });
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    use super::*;
    use crate::agent::test_fixtures::test_agent_with;
    use crate::agent::InvocationState;
    use crate::error::Error;
    use crate::hooks::Cancel;
    use crate::injection::tests::user_with_tool_result;
    use crate::models::Model;
    use crate::types::{ContentBlock, Role};

    async fn run_hook(agent: &Agent, messages: Vec<Message>) -> Vec<Message> {
        run_hook_event(agent, messages).await.messages
    }

    async fn run_hook_event(agent: &Agent, messages: Vec<Message>) -> BeforeModelCallEvent {
        let model: Arc<dyn Model> = agent.model();
        let mut event = BeforeModelCallEvent {
            agent: agent.clone(),
            model,
            invocation_state: InvocationState::new(),
            projected_input_tokens: None,
            cancel: Cancel::No,
            messages,
            dynamic_trailing_blocks: 0,
        };
        agent.hooks().invoke_callbacks(&mut event).await.unwrap();
        event
    }

    #[test]
    fn has_a_default_name_that_can_be_overridden() {
        assert_eq!(ContextInjector::inject("x").name(), "strands:context-injector");
        assert_eq!(ContextInjector::inject("x").with_name("mine").name(), "mine");
    }

    #[tokio::test]
    async fn folds_render_content_text_into_the_latest_user_message() {
        let agent = test_agent_with("c1", None, Vec::new());
        ContextInjector::inject("injected context")
            .init_agent(&agent)
            .await
            .unwrap();
        let messages = run_hook(&agent, vec![Message::user("hello")]).await;
        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].content.len(), 2);
        assert_eq!(messages[0].content[1], ContentBlock::text("\n\ninjected context"));
    }

    #[tokio::test]
    async fn skips_on_a_non_user_turn_by_default() {
        let agent = test_agent_with("c2", None, Vec::new());
        ContextInjector::inject("ctx").init_agent(&agent).await.unwrap();
        let messages = run_hook(&agent, vec![Message::user("hi"), Message::assistant("there")]).await;
        assert_eq!(messages[0].content.len(), 1);
        assert_eq!(messages[1].content.len(), 1);
    }

    #[tokio::test]
    async fn skips_a_tool_result_user_turn_by_default() {
        let agent = test_agent_with("c3", None, Vec::new());
        ContextInjector::inject("ctx").init_agent(&agent).await.unwrap();
        let messages = run_hook(&agent, vec![Message::user("hi"), user_with_tool_result()]).await;
        assert_eq!(messages[1].content.len(), 1);
    }

    #[tokio::test]
    async fn every_turn_injects_regardless_of_the_latest_role() {
        let agent = test_agent_with("c4", None, Vec::new());
        ContextInjector::inject("ctx")
            .with_trigger(InjectionTrigger::EveryTurn)
            .init_agent(&agent)
            .await
            .unwrap();
        let messages = run_hook(&agent, vec![Message::user("hi"), Message::assistant("there")]).await;
        assert_eq!(messages[0].content.len(), 2, "folded onto the last user message");
        assert_eq!(messages[1].content.len(), 1);
    }

    #[tokio::test]
    async fn exposes_app_state_and_the_agent_to_render_content() {
        let agent = test_agent_with("c5", None, Vec::new());
        agent.state().set("who", "world");
        ContextInjector::new(|ctx: &InjectionContext| {
            let who = ctx
                .agent
                .state()
                .get("who")
                .and_then(|v| v.as_str().map(str::to_owned))
                .unwrap_or_default();
            let count = ctx.messages.len();
            Box::pin(async move { Ok(Some(format!("hello {who} ({count} messages)"))) })
        })
        .init_agent(&agent)
        .await
        .unwrap();
        let messages = run_hook(&agent, vec![Message::user("hi")]).await;
        assert_eq!(
            messages[0].content[1],
            ContentBlock::text("\n\nhello world (1 messages)")
        );
    }

    #[tokio::test]
    async fn fails_open_when_render_content_errors() {
        let agent = test_agent_with("c6", None, Vec::new());
        ContextInjector::new(|_ctx: &InjectionContext| Box::pin(async { Err(Error::other("boom")) }))
            .init_agent(&agent)
            .await
            .unwrap();
        let messages = run_hook(&agent, vec![Message::user("hi")]).await;
        assert_eq!(messages[0].content.len(), 1);
    }

    #[tokio::test]
    async fn empty_or_absent_text_is_a_no_op() {
        let agent = test_agent_with("c7", None, Vec::new());
        ContextInjector::new(|_ctx: &InjectionContext| Box::pin(async { Ok(Some("  ".to_owned())) }))
            .init_agent(&agent)
            .await
            .unwrap();
        ContextInjector::new(|_ctx: &InjectionContext| Box::pin(async { Ok(None) }))
            .with_name("second")
            .init_agent(&agent)
            .await
            .unwrap();
        let messages = run_hook(&agent, vec![Message::user("hi")]).await;
        assert_eq!(messages[0].content.len(), 1);
    }

    #[tokio::test]
    async fn a_custom_trigger_predicate_decides() {
        let agent = test_agent_with("c8", None, Vec::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&calls);
        ContextInjector::inject("ctx")
            .with_trigger_fn(move |ctx| {
                seen.fetch_add(1, Ordering::SeqCst);
                ctx.messages.len() > 1
            })
            .init_agent(&agent)
            .await
            .unwrap();
        let messages = run_hook(&agent, vec![Message::user("hi")]).await;
        assert_eq!(messages[0].content.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn no_user_message_means_no_injection() {
        let agent = test_agent_with("c9", None, Vec::new());
        ContextInjector::inject("ctx")
            .with_trigger(InjectionTrigger::EveryTurn)
            .init_agent(&agent)
            .await
            .unwrap();
        let messages = run_hook(&agent, vec![Message::assistant("hello")]).await;
        assert_eq!(messages[0].content.len(), 1);
    }

    #[tokio::test]
    async fn text_starting_with_a_newline_is_appended_without_a_separator() {
        let mut messages = vec![Message::user("hi")];
        assert!(fold_into_last_user_message(&mut messages, "\ninjected").folded);
        assert_eq!(messages[0].content[1], ContentBlock::text("\ninjected"));
    }

    #[tokio::test]
    async fn the_hook_reports_per_call_trailing_blocks_and_accumulates_across_injectors() {
        let agent = test_agent_with("c10", None, Vec::new());
        ContextInjector::inject("one").init_agent(&agent).await.unwrap();
        ContextInjector::inject("two")
            .with_name("second")
            .init_agent(&agent)
            .await
            .unwrap();
        let event = run_hook_event(&agent, vec![Message::user("hi")]).await;

        assert_eq!(event.dynamic_trailing_blocks, 2);
        assert_eq!(event.messages[0].content.len(), 3);
    }

    #[tokio::test]
    async fn injected_text_reaches_the_model_and_history_is_untouched() {
        use crate::test_support::{MockMessageModel, MockResponse};

        let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("ok"))]);
        let agent = Agent::builder()
            .model(Arc::clone(&model) as Arc<dyn Model>)
            .plugin(ContextInjector::inject("ephemeral note"))
            .build()
            .unwrap();

        agent.invoke("hello").await.unwrap();

        let (sent, _) = model.calls().remove(0);
        let last = sent.last().unwrap();
        assert_eq!(last.role, Role::User);
        assert_eq!(last.content.len(), 2);
        assert_eq!(last.content[1], ContentBlock::text("\n\nephemeral note"));

        let history = agent.messages();
        assert_eq!(
            history[0].content.len(),
            1,
            "history keeps only the user's own text"
        );
        assert_eq!(history[0].text(), "hello");
    }
}
