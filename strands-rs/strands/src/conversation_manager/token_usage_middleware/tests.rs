//! Ported from strands-ts/src/conversation-manager/__tests__/token-usage-middleware.test.ts

use std::sync::Arc;

use super::*;
use crate::agent::Agent;
use crate::experimental::context_manager::ContextManagerPreset;
use crate::models::model::{BaseModelConfig, Model};
use crate::models::streaming::Usage;
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::types::agent::{InvocationState, LocalAgent};
use crate::types::messages::{ContentBlock, MessageMetadata};

fn mock_model(context_window_limit: Option<u64>) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    model.update_config(BaseModelConfig {
        context_window_limit,
        ..BaseModelConfig::default()
    });
    model
}

fn hello() -> Message {
    Message::new(Role::User, vec![TextBlock::new("hello").into()])
}

fn context_with(model: Arc<dyn Model>, messages: Vec<Message>, projected: Option<u64>) -> InvokeModelContext {
    let agent = LocalAgent::new(Arc::clone(&model));
    InvokeModelContext::new(
        agent.tool_agent(),
        model,
        messages,
        None,
        Vec::new(),
        None,
        InvocationState::new(),
        projected,
        None,
    )
}

fn context(projected: Option<u64>) -> InvokeModelContext {
    context_with(mock_model(Some(200_000)), vec![hello()], projected)
}

fn last_text(message: &Message) -> &str {
    match message.content().last() {
        Some(ContentBlock::Text(text)) => text.text(),
        other => panic!("expected a text block, got {other:?}"),
    }
}

fn status(context: &InvokeModelContext) -> &str {
    last_text(context.messages.last().unwrap())
}

mod create_token_usage_middleware {
    use super::*;

    #[test]
    fn returns_context_unchanged_when_projected_input_tokens_is_not_set() {
        let context = context(None);
        let before = context.messages.clone();
        let result = add_context_status(context);
        assert_eq!(result.messages, before);
        assert_eq!(result.dynamic_trailing_blocks, None);
    }

    #[test]
    fn returns_context_unchanged_when_messages_are_empty() {
        let result = add_context_status(context_with(mock_model(Some(200_000)), Vec::new(), Some(50_000)));
        assert!(result.messages.is_empty());
        assert_eq!(result.dynamic_trailing_blocks, None);
    }

    #[test]
    fn appends_context_status_to_last_message_content() {
        let result = add_context_status(context(Some(50_000)));
        assert_eq!(result.messages.len(), 1);
        assert_eq!(result.messages[0].content().len(), 2);
        let text = status(&result);
        assert!(text.contains("<context-status>"));
        assert!(text.contains("25.0%"));
        assert!(text.contains("<remaining>"));
        assert!(text.contains("</context-status>"));
    }

    #[test]
    fn uses_the_effective_context_model_limit() {
        let result = add_context_status(context_with(mock_model(Some(10_000)), vec![hello()], Some(8_000)));
        assert!(status(&result).contains("8,000 / 10,000 tokens (80.0%)"));
        assert!(status(&result).contains("<remaining>~2,000 tokens</remaining>"));
    }

    #[test]
    fn reports_the_status_line_as_one_per_call_block() {
        assert_eq!(
            add_context_status(context(Some(50_000))).dynamic_trailing_blocks,
            Some(1)
        );
    }

    #[test]
    fn reports_no_per_call_trailing_blocks_when_the_status_line_lands_in_an_assistant_message() {
        let messages = vec![
            Message::new(Role::User, vec![TextBlock::new("durable ask").into()]),
            Message::new(Role::Assistant, vec![TextBlock::new("reply").into()]),
        ];
        let result = add_context_status(context_with(mock_model(Some(200_000)), messages, Some(50_000)));
        let last = result.messages.last().unwrap();
        assert_eq!(last.role(), Role::Assistant);
        assert!(last_text(last).contains("<context-status>"));
        assert_eq!(result.dynamic_trailing_blocks.unwrap_or(0), 0);
    }

    #[test]
    fn reports_no_per_call_trailing_blocks_when_no_status_line_is_appended() {
        assert_eq!(
            add_context_status(context(None))
                .dynamic_trailing_blocks
                .unwrap_or(0),
            0
        );
    }

    #[test]
    fn does_not_mutate_the_original_messages_array() {
        let original = hello();
        let context = context_with(mock_model(Some(200_000)), vec![original.clone()], Some(50_000));
        let result = add_context_status(context);
        assert_eq!(original.content().len(), 1);
        assert_eq!(result.messages[0].content().len(), 2);
    }

    #[test]
    fn uses_default_context_window_limit_when_model_config_has_no_limit() {
        let result = add_context_status(context_with(mock_model(None), vec![hello()], Some(100_000)));
        let text = status(&result);
        assert!(text.contains("<context-status>"));
        assert!(text.contains("50.0%"));
        assert!(text.contains("200,000"));
    }

    #[test]
    fn formats_token_counts_the_same_regardless_of_the_host_locale() {
        let result = add_context_status(context(Some(100_000)));
        assert!(status(&result).contains("<used>100,000 / 200,000 tokens (50.0%)</used>"));
        assert!(status(&result).contains("<remaining>~100,000 tokens</remaining>"));
    }

    #[test]
    fn preserves_message_metadata() {
        let metadata = MessageMetadata::new().with_usage(Usage::new(10, 5, 15));
        let message = hello().with_metadata(metadata.clone());
        let result = add_context_status(context_with(
            mock_model(Some(200_000)),
            vec![message],
            Some(50_000),
        ));
        assert_eq!(result.messages[0].metadata(), Some(&metadata));
    }

    #[test]
    fn reports_correct_remaining_tokens() {
        let result = add_context_status(context_with(
            mock_model(Some(100_000)),
            vec![hello()],
            Some(80_000),
        ));
        assert!(status(&result).contains("80.0%"));
        assert!(status(&result).contains("<remaining>~20,000 tokens</remaining>"));
    }

    #[test]
    fn rounds_percent_ties_up_like_to_fixed() {
        assert_eq!(percent(1, 400), "0.3");
        assert_eq!(percent(300, 200_000), "0.1");
        assert_eq!(percent(0, 1000), "0.0");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(grouped(999), "999");
    }
}

mod token_usage_middleware_integration {
    use super::*;

    fn turns() -> Arc<MockMessageModel> {
        let model = mock_model(Some(100_000));
        for (reply, usage) in [
            ("First response", Usage::new(1000, 200, 1200)),
            ("Second response", Usage::new(2000, 300, 2300)),
        ] {
            model.add_turn_with(
                vec![TextBlock::new(reply).into()],
                MockTurnOptions {
                    usage: Some(usage),
                    ..MockTurnOptions::default()
                },
            );
        }
        model
    }

    fn agent(model: &Arc<MockMessageModel>, agentic: bool) -> Agent {
        Agent::builder()
            .model(Arc::clone(model))
            .maybe_context_manager(agentic.then_some(ContextManagerPreset::Agentic))
            .printer(false)
            .build()
            .unwrap()
    }

    fn last_request_text(model: &MockMessageModel, call: usize) -> String {
        let messages = model.requests().remove(call).messages;
        last_text(messages.last().unwrap()).to_owned()
    }

    #[tokio::test]
    async fn injects_context_status_into_messages_sent_to_model_on_second_call() {
        let model = turns();
        let mut agent = agent(&model, true);
        agent.invoke("First message").await.unwrap();
        agent.invoke("Second message").await.unwrap();
        let text = last_request_text(&model, 1);
        assert!(text.contains("<context-status>"));
        assert!(text.contains("100,000"));
        assert!(text.contains("<remaining>"));
    }

    #[tokio::test]
    async fn injects_on_first_call_when_token_estimation_succeeds() {
        let model = turns();
        let mut agent = agent(&model, true);
        agent.invoke("First message").await.unwrap();
        let text = last_request_text(&model, 0);
        assert!(text.contains("<context-status>"));
        assert!(text.contains("100,000"));
    }

    #[tokio::test]
    async fn does_not_register_middleware_when_context_manager_is_not_agentic() {
        let model = turns();
        let mut agent = agent(&model, false);
        agent.invoke("First message").await.unwrap();
        agent.invoke("Second message").await.unwrap();
        assert!(!last_request_text(&model, 1).contains("<context-status>"));
    }
}
