//! The injection engine (`strands-ts/src/injection/message-injection.ts`): folds just-in-time
//! text into the model input for one call.

use std::sync::Arc;

use futures::future::BoxFuture;

use crate::agent::Agent;
use crate::error::Result;
use crate::types::{ContentBlock, Message, Role};

use super::types::{InjectionContext, InjectionTrigger, Trigger};

/// Renders the text to inject for one model call; `None` (or blank) skips this call.
pub type RenderFn =
    Arc<dyn for<'a> Fn(&'a InjectionContext) -> BoxFuture<'a, Result<Option<String>>> + Send + Sync>;

/// What [`fold_into_last_user_message`] did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FoldOutcome {
    /// Whether a user message was found and the text appended to it.
    pub folded: bool,
    /// Trailing blocks of the conversation's **last** message that are now per-call content: 1
    /// when the fold landed on the last message, 0 otherwise. A provider keeps that many blocks
    /// out of its cached prefix (`StreamOptions::dynamic_trailing_blocks`).
    pub trailing_blocks: usize,
}

/// A fresh user ask: the last message is from the user and has no tool-result blocks.
pub fn is_user_turn(messages: &[Message]) -> bool {
    match messages.last() {
        Some(last) => {
            last.role == Role::User
                && !last
                    .content
                    .iter()
                    .any(|block| matches!(block, ContentBlock::ToolResult(_)))
        }
        None => false,
    }
}

/// Appends `text` as a text block to the most recent user message, in place.
///
/// The text is always appended: a tool result has to stay the first block of the turn answering a
/// tool use, and a trailing run is the only placement a provider can keep out of its cached
/// prefix. A `\n\n` separator is inserted when the target message already has content and `text`
/// does not start with a newline (some providers concatenate adjacent text blocks).
pub fn fold_into_last_user_message(messages: &mut [Message], text: &str) -> FoldOutcome {
    let last_index = messages.len().saturating_sub(1);
    let Some((index, target)) = messages
        .iter_mut()
        .enumerate()
        .rev()
        .find(|(_, message)| message.role == Role::User)
    else {
        return FoldOutcome::default();
    };
    let separator = if !target.content.is_empty() && !text.starts_with('\n') {
        "\n\n"
    } else {
        ""
    };
    target
        .content
        .push(ContentBlock::text(format!("{separator}{text}")));
    FoldOutcome {
        folded: true,
        trailing_blocks: usize::from(index == last_index),
    }
}

/// A trigger plus a renderer: the delivery primitive behind
/// [`ContextInjector`](crate::plugins::ContextInjector) (TS `createInjectionMiddleware`).
///
/// [`apply`](Self::apply) rewrites the per-call message list only, so the injected text never
/// enters durable history. Everything that skips — the trigger not firing, the renderer returning
/// nothing, or the renderer failing — leaves the messages untouched so the model call proceeds
/// (fail open).
#[derive(Clone)]
pub struct MessageInjection {
    trigger: Trigger,
    render: RenderFn,
}

impl MessageInjection {
    /// Injection whose text is produced per call by `render`.
    pub fn new<F>(render: F) -> Self
    where
        F: for<'a> Fn(&'a InjectionContext) -> BoxFuture<'a, Result<Option<String>>> + Send + Sync + 'static,
    {
        Self {
            trigger: Trigger::default(),
            render: Arc::new(render),
        }
    }

    /// Injection of a fixed string.
    pub fn text(text: impl Into<String>) -> Self {
        let text = text.into();
        Self::new(move |_context| {
            let text = text.clone();
            Box::pin(async move { Ok(Some(text)) })
        })
    }

    /// Set the built-in trigger.
    pub fn with_trigger(mut self, trigger: InjectionTrigger) -> Self {
        self.trigger = Trigger::builtin(trigger);
        self
    }

    /// Use a custom predicate as the trigger.
    pub fn with_trigger_fn<F>(mut self, predicate: F) -> Self
    where
        F: Fn(&InjectionContext) -> bool + Send + Sync + 'static,
    {
        self.trigger = Trigger::predicate(predicate);
        self
    }

    /// Render and fold the text into `messages` for this one call.
    pub async fn apply(&self, agent: &Agent, messages: &mut [Message]) -> FoldOutcome {
        let context = InjectionContext {
            messages: messages.to_vec(),
            agent: agent.clone(),
        };
        if !self.trigger.should_inject(&context) {
            return FoldOutcome::default();
        }
        let text = match (self.render)(&context).await {
            Ok(Some(text)) => text,
            Ok(None) => return FoldOutcome::default(),
            Err(error) => {
                tracing::warn!(reason = %error, "injection renderer failed | skipping injection");
                return FoldOutcome::default();
            }
        };
        if text.trim().is_empty() {
            return FoldOutcome::default();
        }
        fold_into_last_user_message(messages, &text)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::agent::test_fixtures::test_agent_with;
    use crate::error::Error;
    use crate::injection::tests::user_with_tool_result;
    use crate::types::MessageMetadata;

    fn tagged_user(text: &str) -> Message {
        let mut message = Message::user(text);
        message.metadata = Some(MessageMetadata {
            custom: Some(serde_json::Map::from_iter([(
                "keep".to_owned(),
                serde_json::json!("me"),
            )])),
            ..MessageMetadata::default()
        });
        message
    }

    #[test]
    fn appends_the_text_as_a_trailing_text_block_on_the_last_user_message() {
        let mut messages = vec![
            Message::user("original task"),
            Message::assistant("prior step"),
            Message::user("next ask"),
        ];
        let outcome = fold_into_last_user_message(&mut messages, "INJECTED");

        assert_eq!(messages[0].content, vec![ContentBlock::text("original task")]);
        assert_eq!(messages[1].content, vec![ContentBlock::text("prior step")]);
        assert_eq!(
            messages[2].content,
            vec![
                ContentBlock::text("next ask"),
                ContentBlock::text("\n\nINJECTED")
            ]
        );
        assert_eq!(
            outcome,
            FoldOutcome {
                folded: true,
                trailing_blocks: 1
            }
        );
    }

    #[test]
    fn preserves_the_folded_into_message_tracking_id() {
        let mut messages = vec![Message::assistant("prior"), Message::user("ask")];
        let tracking_id = messages[1].tracking_id.clone();
        fold_into_last_user_message(&mut messages, "INJECTED");

        assert_eq!(messages[1].tracking_id, tracking_id);
    }

    #[test]
    fn leaves_every_other_message_untouched() {
        let mut messages = vec![Message::assistant("prior"), Message::user("ask")];
        fold_into_last_user_message(&mut messages, "INJECTED");

        assert_eq!(messages[0].content, vec![ContentBlock::text("prior")]);
        assert_eq!(messages[1].content.len(), 2);
    }

    #[test]
    fn appends_after_the_tool_result_block_when_the_target_is_a_tool_result_turn() {
        let tool_result = user_with_tool_result();
        let mut messages = vec![
            Message::user("task"),
            Message::assistant("thinking"),
            tool_result.clone(),
        ];
        fold_into_last_user_message(&mut messages, "INJECTED");

        assert_eq!(
            messages[2].content,
            vec![
                tool_result.content[0].clone(),
                ContentBlock::text("\n\nINJECTED")
            ]
        );
    }

    #[test]
    fn targets_the_most_recent_user_message_when_several_exist() {
        let mut messages = vec![
            Message::user("first"),
            Message::assistant("a"),
            Message::user("second"),
        ];
        fold_into_last_user_message(&mut messages, "INJECTED");

        assert_eq!(messages[0].content, vec![ContentBlock::text("first")]);
        assert_eq!(messages[2].content.len(), 2);
    }

    #[test]
    fn preserves_message_metadata_on_the_folded_message() {
        let mut messages = vec![tagged_user("ask")];
        fold_into_last_user_message(&mut messages, "INJECTED");

        let custom = messages[0].metadata.as_ref().unwrap().custom.as_ref().unwrap();
        assert_eq!(custom["keep"], serde_json::json!("me"));
    }

    #[test]
    fn returns_the_input_unchanged_when_there_is_no_user_message() {
        let mut messages = vec![Message::assistant("only assistant")];
        let outcome = fold_into_last_user_message(&mut messages, "INJECTED");

        assert_eq!(messages[0].content, vec![ContentBlock::text("only assistant")]);
        assert_eq!(outcome, FoldOutcome::default());
    }

    #[test]
    fn reports_no_per_call_trailing_blocks_when_the_fold_target_is_not_the_last_message() {
        let mut messages = vec![Message::user("ask"), Message::assistant("reply")];
        let outcome = fold_into_last_user_message(&mut messages, "INJECTED");

        assert_eq!(messages[0].content.len(), 2);
        assert_eq!(
            outcome,
            FoldOutcome {
                folded: true,
                trailing_blocks: 0
            }
        );
    }

    #[test]
    fn omits_the_blank_line_separator_when_the_target_message_has_no_content() {
        let mut messages = vec![Message::new(Role::User, Vec::new())];
        fold_into_last_user_message(&mut messages, "INJECTED");

        assert_eq!(messages[0].content, vec![ContentBlock::text("INJECTED")]);
    }

    #[test]
    fn is_user_turn_distinguishes_a_fresh_ask() {
        assert!(is_user_turn(&[Message::assistant("prior"), Message::user("ask")]));
        assert!(!is_user_turn(&[Message::user("task"), user_with_tool_result()]));
        assert!(!is_user_turn(&[Message::user("ask"), Message::assistant("reply")]));
        assert!(!is_user_turn(&[]));
    }

    #[tokio::test]
    async fn folds_rendered_text_into_the_latest_user_message() {
        let agent = test_agent_with("i1", None, Vec::new());
        let mut messages = vec![Message::assistant("prior"), Message::user("ask")];
        let outcome = MessageInjection::text("INJECTED").apply(&agent, &mut messages).await;

        assert_eq!(messages[1].content[1], ContentBlock::text("\n\nINJECTED"));
        assert_eq!(outcome.trailing_blocks, 1);
    }

    #[tokio::test]
    async fn passes_the_conversation_and_the_agent_to_the_renderer() {
        let agent = test_agent_with("i2", None, Vec::new());
        agent.state().set("who", "world");
        let injection = MessageInjection::new(|context: &InjectionContext| {
            let roles: Vec<&str> = context
                .messages
                .iter()
                .map(|message| match message.role {
                    Role::User => "user",
                    Role::Assistant => "assistant",
                })
                .collect();
            let who = context
                .agent
                .state()
                .get("who")
                .and_then(|value| value.as_str().map(str::to_owned))
                .unwrap_or_default();
            let text = format!("{} {who}", roles.join(","));
            Box::pin(async move { Ok(Some(text)) })
        });

        let mut messages = vec![Message::assistant("prior"), Message::user("ask")];
        injection.apply(&agent, &mut messages).await;

        assert_eq!(
            messages[1].content[1],
            ContentBlock::text("\n\nassistant,user world")
        );
    }

    #[tokio::test]
    async fn returns_the_messages_unchanged_when_the_trigger_does_not_fire() {
        let agent = test_agent_with("i3", None, Vec::new());
        let calls = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&calls);
        let injection = MessageInjection::new(move |_context| {
            counted.fetch_add(1, Ordering::SeqCst);
            Box::pin(async { Ok(Some("INJECTED".to_owned())) })
        });

        let mut messages = vec![Message::user("task"), user_with_tool_result()];
        let outcome = injection.apply(&agent, &mut messages).await;

        assert_eq!(messages[1].content.len(), 1);
        assert_eq!(outcome, FoldOutcome::default());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn every_turn_injects_on_a_tool_result_turn_keeping_the_tool_result_first() {
        let agent = test_agent_with("i4", None, Vec::new());
        let injection = MessageInjection::text("INJECTED").with_trigger(InjectionTrigger::EveryTurn);

        let mut messages = vec![Message::user("task"), user_with_tool_result()];
        injection.apply(&agent, &mut messages).await;

        assert!(matches!(messages[1].content[0], ContentBlock::ToolResult(_)));
        assert_eq!(messages[1].content[1], ContentBlock::text("\n\nINJECTED"));
    }

    #[tokio::test]
    async fn a_custom_trigger_predicate_gates_the_injection() {
        let agent = test_agent_with("i5", None, Vec::new());
        let injection =
            MessageInjection::text("INJECTED").with_trigger_fn(|context| context.messages.len() >= 2);

        let mut one = vec![Message::user("ask")];
        assert_eq!(injection.apply(&agent, &mut one).await, FoldOutcome::default());

        let mut two = vec![Message::assistant("prior"), Message::user("ask")];
        assert!(injection.apply(&agent, &mut two).await.folded);
    }

    #[tokio::test]
    async fn blank_or_absent_text_is_a_no_op() {
        let agent = test_agent_with("i6", None, Vec::new());
        let blank = MessageInjection::new(|_context| Box::pin(async { Ok(Some("   ".to_owned())) }));
        let absent = MessageInjection::new(|_context| Box::pin(async { Ok(None) }));

        let mut messages = vec![Message::user("ask")];
        assert_eq!(blank.apply(&agent, &mut messages).await, FoldOutcome::default());
        assert_eq!(absent.apply(&agent, &mut messages).await, FoldOutcome::default());
        assert_eq!(messages[0].content.len(), 1);
    }

    #[tokio::test]
    async fn fails_open_when_the_renderer_errors() {
        let agent = test_agent_with("i7", None, Vec::new());
        let injection = MessageInjection::new(|_context| Box::pin(async { Err(Error::other("boom")) }));

        let mut messages = vec![Message::user("ask")];
        let outcome = injection.apply(&agent, &mut messages).await;

        assert_eq!(outcome, FoldOutcome::default());
        assert_eq!(messages[0].content.len(), 1);
    }

    #[tokio::test]
    async fn accumulates_trailing_blocks_across_producers_on_one_call() {
        let agent = test_agent_with("i8", None, Vec::new());
        let mut messages = vec![Message::user("ask")];

        let first = MessageInjection::text("ONE").apply(&agent, &mut messages).await;
        let second = MessageInjection::text("TWO").apply(&agent, &mut messages).await;

        assert_eq!(first.trailing_blocks + second.trailing_blocks, 2);
        assert_eq!(messages[0].content.len(), 3);
    }

    #[tokio::test]
    async fn emits_no_cache_point_into_the_messages() {
        let agent = test_agent_with("i9", None, Vec::new());
        let mut messages = vec![Message::user("ask")];
        MessageInjection::text("INJECTED").apply(&agent, &mut messages).await;

        assert!(!messages
            .iter()
            .flat_map(|message| &message.content)
            .any(|block| matches!(block, ContentBlock::CachePoint(_))));
    }
}
