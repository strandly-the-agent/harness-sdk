//! Ported from strands-ts/src/conversation-manager/__tests__/context-compression.test.ts.

use std::sync::Arc;

use serde_json::json;

use super::{
    DEFAULT_SUMMARIZATION_PROMPT, MessageTypeFilter, adjust_split_point_for_tool_pairs, as_user_summary,
    find_valid_trim_point, generate_summary, matches_message_type,
};
use crate::conversation_manager::compression::pin_message::{partition_pinned, pin_message};
use crate::error::Result;
use crate::models::model::{AggregatedEvent, BaseModelConfig, BoxStream, Model, ModelRequest};
use crate::models::streaming::ModelStreamEvent;
use crate::test_support::text_result;
use crate::testing::MockMessageModel;
use crate::types::citations::{CitationGeneratedContent, CitationsBlock};
use crate::types::messages::{
    ContentBlock, Message, ReasoningBlock, Role, SystemPrompt, TextBlock, ToolUseBlock,
};

fn text_msg(role: Role, text: &str) -> Message {
    Message::new(role, vec![TextBlock::new(text).into()])
}

fn tool_use_msg(id: &str) -> Message {
    Message::new(
        Role::Assistant,
        vec![ToolUseBlock::new("test", id, json!({})).into()],
    )
}

fn tool_result_msg(id: &str) -> Message {
    Message::new(Role::User, vec![text_result(id, "result").into()])
}

fn text_content(texts: &[&str]) -> Vec<ContentBlock> {
    texts.iter().map(|text| TextBlock::new(*text).into()).collect()
}

mod adjust_split_point_for_tool_pairs {
    use super::*;

    #[test]
    fn returns_split_point_when_message_is_plain_text() {
        let messages = [
            text_msg(Role::User, "hello"),
            text_msg(Role::Assistant, "hi"),
            text_msg(Role::User, "bye"),
        ];
        assert_eq!(adjust_split_point_for_tool_pairs(&messages, 0).unwrap(), 0);
    }

    #[test]
    fn skips_tool_result_messages() {
        let messages = [tool_result_msg("id-1"), text_msg(Role::User, "hello")];
        assert_eq!(adjust_split_point_for_tool_pairs(&messages, 0).unwrap(), 1);
    }

    #[test]
    fn skips_tool_use_without_following_tool_result() {
        let messages = [
            tool_use_msg("id-1"),
            text_msg(Role::Assistant, "no result"),
            text_msg(Role::User, "hello"),
        ];
        assert_eq!(adjust_split_point_for_tool_pairs(&messages, 0).unwrap(), 1);
    }

    #[test]
    fn accepts_tool_use_when_followed_by_tool_result() {
        let messages = [
            tool_use_msg("id-1"),
            tool_result_msg("id-1"),
            text_msg(Role::User, "hello"),
        ];
        assert_eq!(adjust_split_point_for_tool_pairs(&messages, 0).unwrap(), 0);
    }

    #[test]
    fn skips_multiple_consecutive_tool_results() {
        let messages = [
            tool_result_msg("id-1"),
            tool_result_msg("id-2"),
            text_msg(Role::User, "hello"),
        ];
        assert_eq!(adjust_split_point_for_tool_pairs(&messages, 0).unwrap(), 2);
    }

    #[test]
    fn throws_when_no_valid_split_point_exists() {
        let messages = [tool_result_msg("id-1"), tool_result_msg("id-2")];
        let error = adjust_split_point_for_tool_pairs(&messages, 0).unwrap_err();
        assert_eq!(
            error.to_string(),
            "Unable to find valid split point for summarization"
        );
    }

    #[test]
    fn returns_split_point_as_is_when_it_equals_messages_length() {
        let messages = [text_msg(Role::User, "hello"), text_msg(Role::Assistant, "hi")];
        assert_eq!(adjust_split_point_for_tool_pairs(&messages, 2).unwrap(), 2);
    }

    #[test]
    fn returns_split_point_as_is_when_it_exceeds_messages_length() {
        let messages = [text_msg(Role::User, "hello")];
        assert_eq!(adjust_split_point_for_tool_pairs(&messages, 5).unwrap(), 5);
    }
}

mod find_valid_trim_point {
    use super::*;

    #[test]
    fn finds_plain_user_message() {
        let messages = [text_msg(Role::User, "hello"), text_msg(Role::Assistant, "hi")];
        assert_eq!(find_valid_trim_point(&messages, 0), 0);
    }

    #[test]
    fn skips_non_user_messages() {
        let messages = [text_msg(Role::Assistant, "hi"), text_msg(Role::User, "hello")];
        assert_eq!(find_valid_trim_point(&messages, 0), 1);
    }

    #[test]
    fn skips_tool_result_user_messages() {
        let messages = [tool_result_msg("id-1"), text_msg(Role::User, "hello")];
        assert_eq!(find_valid_trim_point(&messages, 0), 1);
    }

    #[test]
    fn returns_messages_length_when_no_valid_point_found() {
        let messages = [text_msg(Role::Assistant, "hi"), tool_result_msg("id-1")];
        assert_eq!(find_valid_trim_point(&messages, 0), messages.len());
    }

    #[test]
    fn respects_start_index() {
        let messages = [
            text_msg(Role::User, "skip"),
            text_msg(Role::Assistant, "hi"),
            text_msg(Role::User, "find"),
        ];
        assert_eq!(find_valid_trim_point(&messages, 1), 2);
    }

    #[test]
    fn returns_0_for_empty_messages() {
        assert_eq!(find_valid_trim_point(&[], 0), 0);
    }
}

mod matches_message_type {
    use super::*;

    #[test]
    fn all_always_matches() {
        for message in [
            text_msg(Role::User, "hello"),
            tool_use_msg("id-1"),
            tool_result_msg("id-1"),
        ] {
            assert!(matches_message_type(&message, MessageTypeFilter::All));
        }
    }

    #[test]
    fn tools_matches_tool_messages() {
        assert!(matches_message_type(
            &tool_use_msg("id-1"),
            MessageTypeFilter::Tools
        ));
        assert!(matches_message_type(
            &tool_result_msg("id-1"),
            MessageTypeFilter::Tools
        ));
    }

    #[test]
    fn tools_does_not_match_plain_text() {
        assert!(!matches_message_type(
            &text_msg(Role::User, "hello"),
            MessageTypeFilter::Tools
        ));
        assert!(!matches_message_type(
            &text_msg(Role::Assistant, "hi"),
            MessageTypeFilter::Tools
        ));
    }

    #[test]
    fn messages_matches_plain_text() {
        assert!(matches_message_type(
            &text_msg(Role::User, "hello"),
            MessageTypeFilter::Messages
        ));
        assert!(matches_message_type(
            &text_msg(Role::Assistant, "hi"),
            MessageTypeFilter::Messages
        ));
    }

    #[test]
    fn messages_does_not_match_tool_messages() {
        assert!(!matches_message_type(
            &tool_use_msg("id-1"),
            MessageTypeFilter::Messages
        ));
        assert!(!matches_message_type(
            &tool_result_msg("id-1"),
            MessageTypeFilter::Messages
        ));
    }
}

mod partition_pinned {
    use super::*;

    fn three() -> Vec<Message> {
        vec![
            text_msg(Role::User, "a"),
            text_msg(Role::Assistant, "b"),
            text_msg(Role::User, "c"),
        ]
    }

    #[test]
    fn separates_pinned_and_unpinned_messages() {
        let mut messages = three();
        pin_message(&mut messages, 1);
        let (pinned, unpinned) = partition_pinned(&messages, 0, 3);
        assert_eq!((pinned.len(), unpinned.len()), (1, 2));
    }

    #[test]
    fn respects_range_end() {
        let mut messages = three();
        pin_message(&mut messages, 0);
        pin_message(&mut messages, 2);
        let (pinned, unpinned) = partition_pinned(&messages, 0, 2);
        assert_eq!((pinned.len(), unpinned.len()), (1, 1));
    }
}

mod generate_summary {
    use super::*;

    fn mock_model(content: Vec<ContentBlock>) -> Arc<MockMessageModel> {
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(content);
        model
    }

    async fn summarize(model: &Arc<MockMessageModel>, messages: &[Message], prompt: Option<&str>) -> Message {
        let dyn_model: Arc<dyn Model> = Arc::clone(model) as Arc<dyn Model>;
        generate_summary(messages, &dyn_model, prompt, None)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn returns_a_user_role_message() {
        let model = mock_model(text_content(&["Summary of conversation"]));
        let messages = [text_msg(Role::User, "hello"), text_msg(Role::Assistant, "hi")];
        assert_eq!(summarize(&model, &messages, None).await.role(), Role::User);
    }

    #[tokio::test]
    async fn passes_the_default_system_prompt() {
        let model = mock_model(text_content(&["Summary"]));
        summarize(&model, &[text_msg(Role::User, "hello")], None).await;
        let request = model.requests().remove(0);
        assert_eq!(
            request.system_prompt,
            Some(SystemPrompt::from(DEFAULT_SUMMARIZATION_PROMPT))
        );
    }

    #[tokio::test]
    async fn passes_a_custom_system_prompt() {
        let model = mock_model(text_content(&["Summary"]));
        summarize(&model, &[text_msg(Role::User, "hello")], Some("Custom prompt")).await;
        let request = model.requests().remove(0);
        assert_eq!(request.system_prompt, Some(SystemPrompt::from("Custom prompt")));
    }

    #[tokio::test]
    async fn appends_summarization_request_message() {
        let model = mock_model(text_content(&["Summary"]));
        let original = [text_msg(Role::User, "hello"), text_msg(Role::Assistant, "hi")];
        summarize(&model, &original, None).await;
        let passed = model.requests().remove(0).messages;
        assert_eq!(passed.len(), 3);
        assert_eq!(
            passed[2].content(),
            text_content(&["Please summarize this conversation."])
        );
    }

    #[tokio::test]
    async fn does_not_mutate_original_messages() {
        let model = mock_model(text_content(&["Summary"]));
        let original = vec![text_msg(Role::User, "hello"), text_msg(Role::Assistant, "hi")];
        let before = original.clone();
        summarize(&model, &original, None).await;
        assert_eq!(original, before);
    }

    /// A model whose aggregated stream ends without a result.
    struct SilentModel;

    impl Model for SilentModel {
        fn stream(&self, _request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
            Box::pin(futures::stream::empty())
        }

        fn stream_aggregated(&self, _request: ModelRequest) -> BoxStream<'static, Result<AggregatedEvent>> {
            Box::pin(futures::stream::empty())
        }

        fn get_config(&self) -> BaseModelConfig {
            BaseModelConfig::default()
        }

        fn update_config(&self, _config: BaseModelConfig) {}
    }

    #[tokio::test]
    async fn throws_if_model_returns_no_response() {
        let model: Arc<dyn Model> = Arc::new(SilentModel);
        let error = generate_summary(&[text_msg(Role::User, "hello")], &model, None, None)
            .await
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Failed to generate summary: no response from model"
        );
    }

    #[tokio::test]
    async fn drops_reasoning_blocks_from_the_summary() {
        let reasoning = ReasoningBlock::new().with_text("thinking").with_signature("sig");
        let model = mock_model(vec![reasoning.into(), TextBlock::new("Summary").into()]);
        let result = summarize(&model, &[text_msg(Role::User, "hello")], None).await;
        assert_eq!(result.role(), Role::User);
        assert_eq!(result.content(), text_content(&["Summary"]));
    }
}

mod as_user_summary {
    use super::*;

    fn cited(texts: &[&str]) -> ContentBlock {
        let content = texts
            .iter()
            .map(|text| CitationGeneratedContent::new(*text))
            .collect();
        CitationsBlock::new(Vec::new(), content).into()
    }

    #[test]
    fn keeps_only_text_blocks() {
        let message = Message::new(
            Role::Assistant,
            vec![
                ReasoningBlock::new()
                    .with_text("thinking")
                    .with_signature("sig")
                    .into(),
                TextBlock::new("Summary").into(),
                ToolUseBlock::new("test", "id-1", json!({})).into(),
            ],
        );
        let result = as_user_summary(&message).unwrap();
        assert_eq!(result.role(), Role::User);
        assert_eq!(result.content(), text_content(&["Summary"]));
    }

    #[test]
    fn unwraps_text_from_citations_blocks() {
        let message = Message::new(Role::Assistant, vec![cited(&["Cited summary"])]);
        let result = as_user_summary(&message).unwrap();
        assert_eq!(result.role(), Role::User);
        assert_eq!(result.content(), text_content(&["Cited summary"]));
    }

    #[test]
    fn skips_blank_cited_text() {
        let message = Message::new(Role::Assistant, vec![cited(&["Cited summary", ""])]);
        assert_eq!(
            as_user_summary(&message).unwrap().content(),
            text_content(&["Cited summary"])
        );
    }

    #[test]
    fn throws_when_the_reply_has_no_text() {
        let message = Message::new(
            Role::Assistant,
            vec![ReasoningBlock::new().with_text("thinking").into(), cited(&[""])],
        );
        assert!(
            as_user_summary(&message)
                .unwrap_err()
                .to_string()
                .contains("no text")
        );
    }
}
