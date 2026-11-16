//! Ported from strands-ts/src/models/__tests__/model-proxy.test.ts.
//!
//! Gaps: `toBe` identity checks (`passes ... through as they are`) compare by value; Rust
//! requests own their messages and the caller's copy can't change.

use std::sync::{Arc, Mutex};

use futures::StreamExt;

use super::*;
use crate::agent::InvocationContext;
use crate::error::Error;
use crate::models::model::{BaseModelConfig, StreamAggregatedResult};
use crate::models::model_info::{ModelCapabilities, ModelInfo};
use crate::models::streaming::{ModelStreamEvent, Usage};
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::types::messages::{InjectedBlock, Role, SummaryBlock, UnknownBlock};
use crate::types::tools::ToolResultStatus;

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn injected(text: &str) -> ContentBlock {
    InjectedBlock::new(text, "test").into()
}

fn model_with_usage(usage: Option<Usage>) -> MockMessageModel {
    let model = MockMessageModel::new();
    model.add_turn_with(
        vec![text("hi")],
        MockTurnOptions {
            usage,
            ..MockTurnOptions::default()
        },
    );
    model
}

fn request(messages: Vec<Message>) -> ModelRequest {
    ModelRequest {
        messages,
        ..ModelRequest::default()
    }
}

async fn drain<M: Model + ?Sized>(
    model: &M,
    request: ModelRequest,
    context: Option<InvocationContext>,
) -> crate::error::Result<(Vec<AggregatedEvent>, StreamAggregatedResult)> {
    let mut events = Vec::new();
    let mut stream = ModelProxy::new(model).stream_aggregated(request, context);
    while let Some(item) = stream.next().await {
        match item? {
            AggregatedEvent::Done(result) => return Ok((events, *result)),
            event => events.push(event),
        }
    }
    Err(Error::other("no result"))
}

/// The messages the model received for `messages`.
async fn sent(model: &MockMessageModel, messages: Vec<Message>) -> Vec<Message> {
    drain(model, request(messages), None).await.unwrap();
    model.requests().pop().unwrap().messages
}

fn usage_of(context: &InvocationContext) -> (u32, u64, u64, u64) {
    let usage = context.usage();
    let tokens = usage.tokens();
    (
        usage.turns_taken(),
        tokens.input_tokens(),
        tokens.output_tokens(),
        tokens.total_tokens(),
    )
}

fn with_capabilities(model: &MockMessageModel, capabilities: ModelCapabilities) {
    model.update_config(BaseModelConfig {
        model_info: Some(ModelInfo {
            capabilities: Some(capabilities),
            ..ModelInfo::default()
        }),
        ..BaseModelConfig::default()
    });
}

mod usage_recording {
    use super::*;

    #[tokio::test]
    async fn accumulates_a_clean_call_usage_into_the_invocation_context_total() {
        let context = InvocationContext::new(None, None, false);
        let model = model_with_usage(Some(Usage::new(10, 5, 15)));
        drain(&model, request(vec![user("go")]), Some(context.clone()))
            .await
            .unwrap();
        assert_eq!(usage_of(&context), (0, 10, 5, 15));
    }

    #[tokio::test]
    async fn accumulates_across_calls_that_share_one_invocation_context() {
        let context = InvocationContext::new(None, None, false);
        let first = model_with_usage(Some(Usage::new(10, 5, 15)));
        let second = model_with_usage(Some(Usage::new(20, 10, 30)));
        drain(&first, request(vec![user("go")]), Some(context.clone()))
            .await
            .unwrap();
        drain(&second, request(vec![user("again")]), Some(context.clone()))
            .await
            .unwrap();
        assert_eq!(usage_of(&context), (0, 30, 15, 45));
    }

    #[tokio::test]
    async fn records_nothing_when_the_model_call_throws() {
        let context = InvocationContext::new(None, None, false);
        let model = MockMessageModel::new();
        model.add_error(|| Error::other("boom"));
        let error = drain(&model, request(vec![user("go")]), Some(context.clone()))
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "boom");
        assert_eq!(usage_of(&context), (0, 0, 0, 0));
    }

    #[tokio::test]
    async fn records_nothing_when_the_model_reports_no_usage() {
        let context = InvocationContext::new(None, None, false);
        drain(
            &model_with_usage(None),
            request(vec![user("go")]),
            Some(context.clone()),
        )
        .await
        .unwrap();
        assert_eq!(usage_of(&context), (0, 0, 0, 0));
    }

    #[tokio::test]
    async fn is_a_no_op_when_called_without_an_invocation_context() {
        let model = model_with_usage(Some(Usage::new(10, 5, 15)));
        let (_, result) = drain(&model, request(vec![user("go")]), None).await.unwrap();
        assert_eq!(
            result.metadata().and_then(|m| m.usage()),
            Some(&Usage::new(10, 5, 15))
        );
    }
}

#[tokio::test]
async fn yields_the_same_events_as_the_underlying_model() {
    let build = || model_with_usage(Some(Usage::new(1, 2, 3)));
    let direct_model = build();
    let mut direct = Vec::new();
    let mut stream = direct_model.stream_aggregated(request(vec![user("go")]));
    while let Some(item) = stream.next().await {
        direct.push(item.unwrap());
    }
    let (events, result) = drain(
        &build(),
        request(vec![user("go")]),
        Some(InvocationContext::new(None, None, false)),
    )
    .await
    .unwrap();
    let Some(AggregatedEvent::Done(direct_result)) = direct.pop() else {
        panic!("expected a result");
    };
    assert_eq!(events, direct);
    assert_eq!(result.stop_reason(), direct_result.stop_reason());
    assert_eq!(result.message().content(), direct_result.message().content());
    assert_eq!(result.metadata(), direct_result.metadata());
    assert!(events.iter().any(|event| matches!(
        event,
        AggregatedEvent::ModelStreamEvent(ModelStreamEvent::Metadata(_))
    )));
}

#[tokio::test]
async fn gives_a_tool_result_whose_content_is_all_unknown_a_placeholder_text() {
    let unknown = ToolResultContent::Unknown(UnknownBlock::new(
        serde_json::json!({"toolReference": {"name": "shell"}})
            .as_object()
            .unwrap()
            .clone(),
    ));
    let kept = ToolResultBlock::new(
        "t-2",
        ToolResultStatus::Success,
        vec!["done".into(), unknown.clone()],
    );
    let messages = vec![
        Message::new(
            Role::User,
            vec![
                ToolResultBlock::new("t-1", ToolResultStatus::Success, vec![unknown.clone(), unknown]).into(),
                kept.clone().into(),
            ],
        )
        .with_tracking_id("msg-1"),
    ];
    let sent = sent(&MockMessageModel::new().tap_turn(), messages).await;
    assert_eq!(
        sent,
        [Message::new(
            Role::User,
            vec![
                ToolResultBlock::new(
                    "t-1",
                    ToolResultStatus::Success,
                    vec![UNSUPPORTED_CONTENT_PLACEHOLDER.into()]
                )
                .into(),
                kept.into(),
            ],
        )
        .with_tracking_id("msg-1")]
    );
}

trait TapTurn {
    fn tap_turn(self) -> Self;
}

impl TapTurn for MockMessageModel {
    fn tap_turn(self) -> Self {
        self.add_turn(vec![text("ok")]);
        self
    }
}

mod injected_blocks {
    use super::*;

    #[tokio::test]
    async fn sends_the_text_apart_from_the_content_before_it_for() {
        let tool_result: ContentBlock =
            ToolResultBlock::new("tool-1", ToolResultStatus::Success, vec!["done".into()]).into();
        let cases = [
            (vec![text("Refactor the tokenizer.")], "reminder", "\n\nreminder"),
            (vec![tool_result], "reminder", "\n\nreminder"),
            (vec![], "reminder", "reminder"),
            (vec![text("Refactor the tokenizer.")], "\nreminder", "\nreminder"),
        ];
        for (content, injected_text, sent_text) in cases {
            let mut with_injected = content.clone();
            with_injected.push(injected(injected_text));
            let messages = vec![Message::new(Role::User, with_injected).with_tracking_id("msg-1")];
            let mut expected = content;
            expected.push(text(sent_text));
            assert_eq!(
                sent(&MockMessageModel::new().tap_turn(), messages).await,
                [Message::new(Role::User, expected).with_tracking_id("msg-1")]
            );
        }
    }

    #[tokio::test]
    async fn sends_each_of_several_injected_blocks_as_its_own_text_block_in_place() {
        let metadata = crate::types::messages::MessageMetadata::default();
        let messages = vec![
            Message::new(
                Role::User,
                vec![text("Refactor the tokenizer."), injected("one"), injected("two")],
            )
            .with_tracking_id("msg-1")
            .with_metadata(metadata.clone()),
        ];
        assert_eq!(
            sent(&MockMessageModel::new().tap_turn(), messages).await,
            [Message::new(
                Role::User,
                vec![text("Refactor the tokenizer."), text("\n\none"), text("\n\ntwo")]
            )
            .with_tracking_id("msg-1")
            .with_metadata(metadata)]
        );
    }

    #[tokio::test]
    async fn leaves_out_the_injected_blocks_of_earlier_user_messages() {
        let answer = Message::new(Role::Assistant, vec![text("Done.")]).with_tracking_id("msg-2");
        let latest = user("Now the docs.");
        let messages = vec![
            Message::new(Role::User, vec![text("Refactor."), injected("reminder")]).with_tracking_id("msg-1"),
            answer.clone(),
            latest.clone(),
        ];
        assert_eq!(
            sent(&MockMessageModel::new().tap_turn(), messages).await,
            [
                Message::new(Role::User, vec![text("Refactor.")]).with_tracking_id("msg-1"),
                answer,
                latest
            ]
        );
    }

    #[tokio::test]
    async fn keeps_the_cache_point_ahead_of_the_latest_injected_text_also_when_an_assistant_message_follows() {
        let model = MockMessageModel::new().tap_turn();
        let messages = vec![Message::new(
            Role::User,
            vec![text("Refactor."), injected("reminder"), text("\n\nper-call text")],
        )];
        let first = ModelRequest {
            dynamic_trailing_blocks: Some(1),
            ..request(messages.clone())
        };
        drain(&model, first, None).await.unwrap();
        let mut followed = messages;
        followed.push(Message::new(Role::Assistant, vec![text("Sure")]));
        drain(&model, request(followed), None).await.unwrap();
        let trailing: Vec<Option<usize>> = model
            .requests()
            .iter()
            .map(|r| r.dynamic_trailing_blocks)
            .collect();
        assert_eq!(trailing, [Some(2), Some(2)]);
    }

    #[tokio::test]
    async fn sends_text_to_a_custom_provider_serving_a_model_that_takes_temporary_messages() {
        let model = MockMessageModel::new().tap_turn();
        model.update_config(BaseModelConfig {
            model_id: Some("claude-opus-5-5".to_owned()),
            ..BaseModelConfig::default()
        });
        let messages = vec![
            Message::new(Role::User, vec![text("Refactor."), injected("reminder")]).with_tracking_id("msg-1"),
        ];
        assert_eq!(
            sent(&model, messages).await,
            [
                Message::new(Role::User, vec![text("Refactor."), text("\n\nreminder")])
                    .with_tracking_id("msg-1")
            ]
        );
    }

    #[tokio::test]
    async fn passes_injected_blocks_through_to_a_model_that_takes_temporary_messages() {
        let model = MockMessageModel::new().tap_turn();
        with_capabilities(
            &model,
            ModelCapabilities {
                temporary_messages: Some(true),
                ..ModelCapabilities::default()
            },
        );
        let messages = vec![
            Message::new(Role::User, vec![text("Refactor."), injected("reminder")]),
            Message::new(Role::Assistant, vec![text("Done.")]),
            user("Now the docs."),
        ];
        assert_eq!(sent(&model, messages.clone()).await, messages);
    }

    #[tokio::test]
    async fn counts_the_messages_it_would_send() {
        struct Counting {
            inner: MockMessageModel,
            counted: Mutex<Vec<(Vec<Message>, CountTokensOptions)>>,
        }
        #[async_trait::async_trait]
        impl Model for Counting {
            fn stream(
                &self,
                request: ModelRequest,
            ) -> BoxStream<'static, crate::error::Result<ModelStreamEvent>> {
                self.inner.stream(request)
            }
            fn get_config(&self) -> BaseModelConfig {
                self.inner.get_config()
            }
            fn update_config(&self, config: BaseModelConfig) {
                self.inner.update_config(config);
            }
            async fn count_tokens(
                &self,
                messages: &[Message],
                options: CountTokensOptions,
            ) -> crate::error::Result<u64> {
                self.counted.lock().unwrap().push((messages.to_vec(), options));
                Ok(0)
            }
        }
        let model = Counting {
            inner: MockMessageModel::new(),
            counted: Mutex::default(),
        };
        let messages = vec![
            Message::new(Role::User, vec![text("Refactor."), injected("reminder")]).with_tracking_id("msg-1"),
        ];
        let options = CountTokensOptions {
            system_prompt: Some("Be brief.".into()),
            ..CountTokensOptions::default()
        };
        ModelProxy::new(&model)
            .count_tokens(&messages, options.clone())
            .await
            .unwrap();
        assert_eq!(
            model.counted.lock().unwrap()[0],
            (
                vec![
                    Message::new(Role::User, vec![text("Refactor."), text("\n\nreminder")])
                        .with_tracking_id("msg-1")
                ],
                options
            )
        );
    }

    #[tokio::test]
    async fn leaves_out_an_injected_block_outside_a_user_message() {
        let prompt = user("go");
        let messages = vec![
            prompt.clone(),
            Message::new(
                Role::Assistant,
                vec![text("Hi"), injected("reminder"), text("Bye")],
            )
            .with_tracking_id("msg-2"),
        ];
        assert_eq!(
            sent(&MockMessageModel::new().tap_turn(), messages).await,
            [
                prompt,
                Message::new(Role::Assistant, vec![text("Hi"), text("Bye")]).with_tracking_id("msg-2")
            ]
        );
    }

    #[tokio::test]
    async fn passes_messages_without_an_injected_block_through_as_they_are() {
        let messages = vec![user("go")];
        assert_eq!(
            sent(&MockMessageModel::new().tap_turn(), messages.clone()).await,
            messages
        );
    }
}

mod summary_blocks {
    use super::*;

    fn summary_history() -> Vec<Message> {
        vec![
            Message::new(
                Role::Assistant,
                vec![SummaryBlock::new("SUMMARY").into(), text("next")],
            ),
            user("go"),
        ]
    }

    #[tokio::test]
    async fn sends_the_summary_text_to_a_model_without_server_side_summary() {
        for server_side_summary in [None, Some(false)] {
            let model = MockMessageModel::new().tap_turn();
            with_capabilities(
                &model,
                ModelCapabilities {
                    server_side_summary,
                    ..ModelCapabilities::default()
                },
            );
            let sent = sent(&model, summary_history()).await;
            assert_eq!(sent[0].content(), [text("SUMMARY"), text("next")]);
        }
    }

    #[tokio::test]
    async fn keeps_the_summary_block_for_a_model_with_server_side_summary() {
        let model = MockMessageModel::new().tap_turn();
        with_capabilities(
            &model,
            ModelCapabilities {
                server_side_summary: Some(true),
                ..ModelCapabilities::default()
            },
        );
        let history = summary_history();
        assert_eq!(sent(&model, history.clone()).await, history);
    }
}

#[tokio::test]
async fn the_agent_sends_and_counts_through_the_proxy() {
    let model = Arc::new(MockMessageModel::new().tap_turn());
    let mut agent = crate::testing::test_agent(model.clone());
    agent
        .invoke(vec![Message::new(
            Role::User,
            vec![text("Refactor."), injected("reminder")],
        )])
        .await
        .unwrap();
    assert_eq!(
        model.requests()[0].messages[0].content(),
        [text("Refactor."), text("\n\nreminder")]
    );
    assert_eq!(agent.messages()[0].content()[1], injected("reminder"));
}
