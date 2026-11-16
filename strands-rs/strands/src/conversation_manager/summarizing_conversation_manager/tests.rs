//! Ported from strands-ts/src/conversation-manager/__tests__/summarizing-conversation-manager.test.ts.

use std::num::NonZeroU32;

use serde_json::json;

use super::*;
use crate::conversation_manager::ConversationManagerPlugin;
use crate::conversation_manager::compression::pin_message::pin_message;
use crate::error::{ContextWindowOverflowError, Error, ModelError};
use crate::hooks::{AfterModelCallEvent, BeforeModelCallEvent};
use crate::models::model::BaseModelConfig;
use crate::models::streaming::Usage;
use crate::plugins::Plugin;
use crate::test_support::text_result;
use crate::testing::{MockMessageModel, MockTurnOptions, TestModelProvider};
use crate::types::agent::{InvocationState, InvokeLimits};
use crate::types::messages::{ContentBlock, Message, Role, SystemPrompt, TextBlock, ToolUseBlock};

fn text_msg(role: Role, text: &str) -> Message {
    Message::new(role, vec![TextBlock::new(text).into()])
}

fn make_messages(count: usize) -> Vec<Message> {
    (0..count)
        .map(|index| {
            let role = if index % 2 == 0 {
                Role::User
            } else {
                Role::Assistant
            };
            text_msg(role, &format!("Message {}", index + 1))
        })
        .collect()
}

fn replying(text: &str) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new(text).into()]);
    model
}

fn summarizing(config: SummarizingConversationManagerConfig) -> SummarizingConversationManager {
    SummarizingConversationManager::new(config).unwrap()
}

fn ratio(summary_ratio: f64, preserve_recent_messages: usize) -> SummarizingConversationManager {
    summarizing(
        SummarizingConversationManagerConfig::builder()
            .summary_ratio(summary_ratio)
            .preserve_recent_messages(preserve_recent_messages)
            .build(),
    )
}

/// An agent whose `aux_model` is `aux`.
fn agent_with(messages: Vec<Message>, aux: &Arc<MockMessageModel>) -> LocalAgent {
    let mut agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
    agent.set_aux_model(Some(Arc::clone(aux) as Arc<dyn Model>));
    agent.set_messages(messages);
    agent
}

fn overflow_error() -> ContextWindowOverflowError {
    ContextWindowOverflowError::new("overflow")
}

async fn reduce(
    manager: &SummarizingConversationManager,
    agent: &mut LocalAgent,
    model: Arc<dyn Model>,
    error: Option<&ContextWindowOverflowError>,
) -> Result<bool> {
    manager
        .reduce(ConversationManagerReduceOptions::new(agent, model, error, None))
        .await
}

fn first_text(message: &Message) -> &str {
    match message.content().first() {
        Some(ContentBlock::Text(text)) => text.text(),
        other => panic!("expected a text block, got {other:?}"),
    }
}

fn texts(messages: &[Message]) -> Vec<&str> {
    messages.iter().map(first_text).collect()
}

mod constructor {
    use super::*;

    #[test]
    fn clamps_summary_ratio_to_0_1_0_8() {
        assert!((ratio(0.0, 10).summary_ratio - 0.1).abs() < f64::EPSILON);
        assert!((ratio(1.0, 10).summary_ratio - 0.8).abs() < f64::EPSILON);
    }

    #[test]
    fn defaults_to_ratio_0_3_and_10_recent_messages() {
        let manager = summarizing(SummarizingConversationManagerConfig::default());
        assert!((manager.summary_ratio - 0.3).abs() < f64::EPSILON);
        assert_eq!(manager.preserve_recent_messages, 10);
        assert_eq!(manager.name(), "strands:summarizing-conversation-manager");
    }
}

mod reduce {
    use super::*;

    #[tokio::test]
    async fn summarizes_oldest_messages_and_replaces_them_with_a_user_role_summary() {
        let model = replying("Summary of conversation");
        let messages = make_messages(20);
        let last_two = messages[18..].to_vec();
        let mut agent = agent_with(messages, &model);
        let error = overflow_error();
        let reduced = reduce(&ratio(0.5, 2), &mut agent, model.clone(), Some(&error))
            .await
            .unwrap();
        assert!(reduced);
        assert_eq!(agent.messages().len(), 11);
        assert_eq!(agent.messages()[0].role(), Role::User);
        assert_eq!(first_text(&agent.messages()[0]), "Summary of conversation");
        assert_eq!(agent.messages()[9..], last_two);
    }

    #[tokio::test]
    async fn assigns_a_durable_tracking_id_to_the_generated_summary_message() {
        let model = replying("Summary of conversation");
        let mut agent = agent_with(make_messages(20), &model);
        let error = overflow_error();
        reduce(&ratio(0.5, 2), &mut agent, model.clone(), Some(&error))
            .await
            .unwrap();
        assert!(!agent.messages()[0].tracking_id().is_empty());
    }

    #[tokio::test]
    async fn uses_the_config_model_over_agent_aux_model_when_provided() {
        let config_model = replying("Config model summary");
        let aux = replying("Aux model summary");
        let manager = summarizing(
            SummarizingConversationManagerConfig::builder()
                .model(Arc::clone(&config_model) as Arc<dyn Model>)
                .summary_ratio(0.5)
                .preserve_recent_messages(2)
                .build(),
        );
        let mut agent = agent_with(make_messages(20), &aux);
        let error = overflow_error();
        reduce(
            &manager,
            &mut agent,
            Arc::new(MockMessageModel::new()),
            Some(&error),
        )
        .await
        .unwrap();
        assert_eq!(first_text(&agent.messages()[0]), "Config model summary");
    }

    #[tokio::test]
    async fn summarizes_with_agent_aux_model_not_the_triggering_model_when_no_config_model_is_set() {
        let aux = replying("Aux model summary");
        let triggering = replying("Triggering model summary");
        let mut agent = agent_with(make_messages(20), &aux);
        let error = overflow_error();
        let reduced = reduce(&ratio(0.5, 2), &mut agent, triggering.clone(), Some(&error))
            .await
            .unwrap();
        assert!(reduced);
        assert_eq!(first_text(&agent.messages()[0]), "Aux model summary");
        assert_eq!(triggering.call_count(), 0);
    }

    #[tokio::test]
    async fn returns_false_when_there_are_not_enough_messages_to_summarize() {
        let model = Arc::new(MockMessageModel::new());
        let manager = summarizing(
            SummarizingConversationManagerConfig::builder()
                .preserve_recent_messages(10)
                .build(),
        );
        let mut agent = agent_with(make_messages(8), &model);
        let error = overflow_error();
        assert!(
            !reduce(&manager, &mut agent, model.clone(), Some(&error))
                .await
                .unwrap()
        );
        assert_eq!(agent.messages().len(), 8);
    }

    // TS also links the overflow as the thrown error's `cause`; the Rust error is the model's.
    #[tokio::test]
    async fn rethrows_model_errors_with_the_overflow_error_as_cause() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| ModelError::new("model failed").into());
        let mut agent = agent_with(make_messages(20), &model);
        let error = overflow_error();
        let thrown = reduce(&ratio(0.5, 2), &mut agent, model.clone(), Some(&error))
            .await
            .unwrap_err();
        assert!(matches!(&thrown, Error::Model(_)), "{thrown:?}");
        assert_eq!(thrown.to_string(), "model failed");
    }

    #[tokio::test]
    async fn passes_the_correct_message_slice_and_system_prompt_to_the_model() {
        let model = replying("Summary");
        let manager = summarizing(
            SummarizingConversationManagerConfig::builder()
                .summary_ratio(0.5)
                .preserve_recent_messages(2)
                .summarization_system_prompt("Custom summarization prompt")
                .build(),
        );
        let messages = make_messages(10);
        let expected_slice = messages[..5].to_vec();
        let mut agent = agent_with(messages, &model);
        let error = overflow_error();
        reduce(&manager, &mut agent, model.clone(), Some(&error))
            .await
            .unwrap();
        let requests = model.requests();
        assert_eq!(requests.len(), 1);
        let called = &requests[0].messages;
        assert_eq!(called.len(), 6);
        assert_eq!(called[..5], expected_slice);
        assert_eq!(called[5].role(), Role::User);
        assert_eq!(first_text(&called[5]), "Please summarize this conversation.");
        assert_eq!(
            requests[0].system_prompt,
            Some(SystemPrompt::from("Custom summarization prompt"))
        );
    }

    #[tokio::test]
    async fn preserve_recent_messages_dominates_when_larger_than_ratio_allows() {
        let model = replying("Summary");
        let mut agent = agent_with(make_messages(20), &model);
        let error = overflow_error();
        assert!(
            reduce(&ratio(0.8, 18), &mut agent, model.clone(), Some(&error))
                .await
                .unwrap()
        );
        assert_eq!(agent.messages().len(), 19);
    }
}

mod request_limits {
    use super::*;

    #[tokio::test]
    async fn adds_the_summarization_usage_to_the_request_total_even_when_the_request_limit_is_used_up() {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn_with(
            vec![TextBlock::new("Summary of conversation").into()],
            MockTurnOptions {
                usage: Some(Usage::new(4, 5, 9)),
                ..MockTurnOptions::default()
            },
        );
        let mut agent = agent_with(make_messages(20), &model);
        let limits = InvokeLimits {
            turns: NonZeroU32::new(1),
            ..InvokeLimits::default()
        };
        let request = InvocationContext::new(Some(limits), None, false);
        request.set_turns_taken(5);
        let options =
            ConversationManagerReduceOptions::new(&mut agent, model.clone(), None, Some(request.clone()));
        assert!(ratio(0.5, 2).reduce(options).await.unwrap());
        assert_eq!(
            agent.messages()[0].content(),
            [ContentBlock::from(TextBlock::new("Summary of conversation"))]
        );
        let usage = request.usage();
        assert_eq!((usage.turns_taken(), usage.tokens()), (5, &Usage::new(4, 5, 9)));
    }
}

mod tool_pair_adjustment {
    use super::*;

    #[tokio::test]
    async fn advances_split_point_past_orphaned_tool_result_and_tool_use_boundaries() {
        let model = replying("Summary");
        let mut messages = vec![
            text_msg(Role::User, "Message 1"),
            text_msg(Role::Assistant, "Message 2"),
            Message::new(
                Role::Assistant,
                vec![ToolUseBlock::new("tool1", "id-1", json!({})).into()],
            ),
            Message::new(Role::User, vec![text_result("id-1", "Result").into()]),
            text_msg(Role::Assistant, "Response after tool"),
        ];
        messages.extend(make_messages(8));
        let mut agent = agent_with(messages, &model);
        let error = overflow_error();
        assert!(
            reduce(&ratio(0.3, 2), &mut agent, model.clone(), Some(&error))
                .await
                .unwrap()
        );
        let second = &agent.messages()[1];
        assert!(
            !second
                .content()
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult(_)))
        );
    }

    #[tokio::test]
    async fn throws_when_no_valid_split_point_exists() {
        let model = Arc::new(MockMessageModel::new());
        let messages = (0..4)
            .map(|index| {
                Message::new(
                    Role::User,
                    vec![text_result(&format!("id-{index}"), &format!("R{index}")).into()],
                )
            })
            .collect();
        let mut agent = agent_with(messages, &model);
        let error = overflow_error();
        let thrown = reduce(&ratio(0.5, 0), &mut agent, model.clone(), Some(&error))
            .await
            .unwrap_err();
        assert_eq!(
            thrown.to_string(),
            "Unable to find valid split point for summarization"
        );
    }
}

async fn plugin_agent(
    manager: SummarizingConversationManager,
    model: Arc<dyn Model>,
    messages: Vec<Message>,
) -> LocalAgent {
    let mut agent = LocalAgent::new(model);
    agent.set_messages(messages);
    let plugin = Arc::new(ConversationManagerPlugin(Arc::new(manager)));
    plugin.init_agent(&mut agent).await.unwrap();
    agent
}

mod base_class_hook_integration {
    use super::*;

    #[tokio::test]
    async fn async_reduce_sets_retry_true_through_the_base_class_await() {
        let model = replying("Summary");
        let mut agent = plugin_agent(ratio(0.5, 2), model.clone(), make_messages(20)).await;
        let hooks = agent.hooks().clone();
        let error = Error::from(overflow_error());
        let mut event = AfterModelCallEvent::new(&mut agent, model, InvocationState::new(), NonZeroU32::MIN)
            .with_error(&error);
        hooks
            .invoke::<AfterModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(event.retry());
        assert_eq!(agent.messages().len(), 11);
    }

    #[tokio::test]
    async fn a_failed_overflow_summary_leaves_retry_unset() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| ModelError::new("model failed").into());
        let mut agent = plugin_agent(ratio(0.5, 2), model.clone(), make_messages(20)).await;
        let hooks = agent.hooks().clone();
        let error = Error::from(overflow_error());
        let mut event = AfterModelCallEvent::new(&mut agent, model, InvocationState::new(), NonZeroU32::MIN)
            .with_error(&error);
        hooks
            .invoke::<AfterModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        assert!(!event.retry());
        assert_eq!(agent.messages().len(), 20);
    }
}

mod reduce_on_threshold {
    use super::*;

    fn proactive(
        model: &Arc<MockMessageModel>,
        ratio: f64,
        preserve: usize,
    ) -> SummarizingConversationManager {
        summarizing(
            SummarizingConversationManagerConfig::builder()
                .model(Arc::clone(model) as Arc<dyn Model>)
                .summary_ratio(ratio)
                .preserve_recent_messages(preserve)
                .proactive_compression(ProactiveCompression::Threshold(0.7))
                .build(),
        )
    }

    async fn before_model_call(manager: SummarizingConversationManager, projected: u64) -> LocalAgent {
        let limited = TestModelProvider::new();
        limited.update_config(BaseModelConfig {
            context_window_limit: Some(1000),
            ..BaseModelConfig::default()
        });
        let limited: Arc<dyn Model> = Arc::new(limited);
        let mut agent = plugin_agent(manager, Arc::clone(&limited), make_messages(20)).await;
        let hooks = agent.hooks().clone();
        let mut event =
            BeforeModelCallEvent::new(&mut agent, limited, InvocationState::new(), Some(projected));
        hooks
            .invoke::<BeforeModelCallEvent<'static>>(&mut event)
            .await
            .unwrap();
        agent
    }

    #[tokio::test]
    async fn summarizes_oldest_messages_when_compression_threshold_is_exceeded() {
        let model = replying("Summary of conversation");
        let agent = before_model_call(proactive(&model, 0.5, 2), 800).await;
        assert_eq!(agent.messages().len(), 11);
        assert_eq!(agent.messages()[0].role(), Role::User);
        assert_eq!(first_text(&agent.messages()[0]), "Summary of conversation");
    }

    #[tokio::test]
    async fn does_not_summarize_when_below_compression_threshold() {
        let model = replying("Summary");
        let agent = before_model_call(proactive(&model, 0.3, 10), 500).await;
        assert_eq!(agent.messages().len(), 20);
    }

    #[tokio::test]
    async fn returns_false_and_does_not_throw_when_summarization_fails() {
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| ModelError::new("model failed").into());
        let agent = before_model_call(proactive(&model, 0.5, 2), 800).await;
        assert_eq!(agent.messages().len(), 20);
    }
}

mod pin_first {
    use super::*;

    fn pinned(ratio: f64, pin_first: usize) -> SummarizingConversationManager {
        summarizing(
            SummarizingConversationManagerConfig::builder()
                .summary_ratio(ratio)
                .preserve_recent_messages(2)
                .pin_first(pin_first)
                .build(),
        )
    }

    #[tokio::test]
    async fn protects_first_n_messages_from_summarization() {
        let model = replying("Summary");
        let messages = vec![
            text_msg(Role::User, "protected-1"),
            text_msg(Role::Assistant, "protected-2"),
            text_msg(Role::User, "summarize-me"),
            text_msg(Role::Assistant, "summarize-me-too"),
            text_msg(Role::User, "recent-1"),
            text_msg(Role::Assistant, "recent-2"),
        ];
        let mut agent = agent_with(messages, &model);
        reduce(&pinned(0.5, 2), &mut agent, model.clone(), None)
            .await
            .unwrap();
        assert_eq!(
            texts(agent.messages()),
            [
                "protected-1",
                "protected-2",
                "Summary",
                "summarize-me-too",
                "recent-1",
                "recent-2"
            ]
        );
    }

    #[tokio::test]
    async fn returns_false_when_all_messages_in_summary_range_are_protected() {
        let model = replying("Summary");
        let mut agent = agent_with(make_messages(6), &model);
        assert!(
            !reduce(&pinned(0.3, 10), &mut agent, model.clone(), None)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn pinned_message_in_middle_survives_summarization() {
        let model = replying("Summary");
        let mut messages = vec![
            text_msg(Role::User, "old-1"),
            text_msg(Role::Assistant, "pinned-middle"),
            text_msg(Role::User, "old-3"),
            text_msg(Role::Assistant, "old-4"),
            text_msg(Role::User, "recent-1"),
            text_msg(Role::Assistant, "recent-2"),
        ];
        pin_message(&mut messages, 1);
        let mut agent = agent_with(messages, &model);
        reduce(&ratio(0.5, 2), &mut agent, model.clone(), None)
            .await
            .unwrap();
        assert_eq!(
            texts(agent.messages()),
            ["pinned-middle", "Summary", "old-4", "recent-1", "recent-2"]
        );
    }
}
