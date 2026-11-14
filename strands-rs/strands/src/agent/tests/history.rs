//! Ported from strands-ts/src/agent/__tests__/agent.test.ts (`_estimateInputTokens`,
//! `injected blocks`, `normalizeToolUseNames` and `Agent._redactLastMessage` cases not in
//! `agent.rs`).

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use serde_json::json;

use crate::agent::{Agent, AgentConfig, InvokeArgs};
use crate::error::{ModelError, Result};
use crate::hooks::BeforeModelCallEvent;
use crate::models::{
    BaseModelConfig, BoxStream, CountTokensOptions, Model, ModelRequest, ModelStreamEvent, Usage,
};
use crate::testing::{MockModel, mock_tool, text_result};
use crate::tools::{ToolResultStatus, ToolSource};
use crate::types::{
    ContentBlock, InjectedBlock, Message, MessageMetadata, Role, ToolResultBlock, ToolResultContent,
    ToolUseBlock,
};

/// A [`MockModel`] whose `count_tokens` answers with a fixed count or fails (TS `vi.spyOn(model, 'countTokens')`).
struct CountingModel {
    inner: Arc<MockModel>,
    count: Option<u64>,
    calls: Mutex<usize>,
}

#[async_trait]
impl Model for CountingModel {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>> {
        self.inner.stream(request)
    }

    fn get_config(&self) -> BaseModelConfig {
        self.inner.get_config()
    }

    fn update_config(&self, config: BaseModelConfig) {
        self.inner.update_config(config);
    }

    async fn count_tokens(&self, _messages: &[Message], _options: CountTokensOptions) -> Result<u64> {
        *self.calls.lock().unwrap() += 1;
        self.count
            .ok_or_else(|| ModelError::new("API unavailable").into())
    }
}

fn counting(inner: &Arc<MockModel>, count: Option<u64>) -> Arc<CountingModel> {
    Arc::new(CountingModel {
        inner: Arc::clone(inner),
        count,
        calls: Mutex::new(0),
    })
}

fn agent(model: Arc<dyn Model>, messages: Vec<Message>, tools: Vec<ToolSource>) -> Agent {
    Agent::new(AgentConfig {
        model: Some(model.into()),
        messages,
        tools,
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

fn capture_projected(agent: &Agent) -> Arc<Mutex<Vec<Option<u64>>>> {
    let seen = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&seen);
    agent.add_hook(move |event: &mut BeforeModelCallEvent<'_>| {
        sink.lock().unwrap().push(event.projected_input_tokens());
    });
    seen
}

fn prior_turn(usage: Usage) -> Vec<Message> {
    vec![
        Message::new(Role::User, vec!["Hi".into()]),
        Message::new(Role::Assistant, vec!["Hello".into()])
            .with_metadata(MessageMetadata::new().with_usage(usage)),
    ]
}

mod estimate_input_tokens {
    use super::*;

    #[tokio::test]
    async fn uses_full_estimation_on_cold_start_with_the_model_count() {
        let inner = Arc::new(MockModel::new());
        inner.add_turn(vec!["Hello".into()]);
        let model = counting(&inner, Some(42));
        let mut agent = agent(model.clone(), Vec::new(), Vec::new());
        let seen = capture_projected(&agent);

        agent.invoke("Hi").await.unwrap();

        assert_eq!(*seen.lock().unwrap(), [Some(42)]);
        assert_eq!(*model.calls.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn uses_known_baseline_when_no_new_messages_after_last_assistant() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hello".into()]);
        let mut agent = agent(model, prior_turn(Usage::new(100, 20, 120)), Vec::new());
        let seen = capture_projected(&agent);

        agent.invoke(InvokeArgs::empty()).await.unwrap();

        assert_eq!(*seen.lock().unwrap(), [Some(120)]);
    }

    #[tokio::test]
    async fn counts_disjoint_provider_cache_reads_in_the_known_baseline() {
        let model = Arc::new(MockModel::new());
        model.add_turn(vec!["Hello".into()]);
        let usage = Usage {
            cache_read_input_tokens: Some(5848),
            ..Usage::new(10, 4, 5862)
        };
        let mut agent = agent(model, prior_turn(usage), Vec::new());
        let seen = capture_projected(&agent);

        agent.invoke(InvokeArgs::empty()).await.unwrap();

        assert_eq!(*seen.lock().unwrap(), [Some(5862)]);
    }

    #[tokio::test]
    async fn returns_undefined_projected_input_tokens_when_estimation_fails() {
        let inner = Arc::new(MockModel::new());
        inner.add_turn(vec!["Hello".into()]);
        let mut agent = agent(counting(&inner, None), Vec::new(), Vec::new());
        let seen = capture_projected(&agent);

        agent.invoke("Hi").await.unwrap();

        assert_eq!(*seen.lock().unwrap(), [None]);
    }

    #[tokio::test]
    async fn estimates_delta_for_new_messages_after_last_assistant() {
        let inner = Arc::new(MockModel::new());
        inner
            .add_turn_with(
                vec![ContentBlock::ToolUse(ToolUseBlock::new(
                    "test",
                    "id-1",
                    json!({}),
                ))],
                None,
                Some(Usage::new(100, 30, 130)),
            )
            .add_turn(vec!["Done".into()]);
        let model = counting(&inner, Some(50));
        let tools = crate::tool_list![mock_tool("test", |ctx| text_result(ctx, "result"))];
        let mut agent = agent(model.clone(), Vec::new(), tools);
        let seen = capture_projected(&agent);

        agent.invoke("Use the tool").await.unwrap();

        assert_eq!(seen.lock().unwrap()[1], Some(180));
        assert!(*model.calls.lock().unwrap() > 0);
    }

    #[tokio::test]
    async fn uses_baseline_from_prior_invocation_on_second_invoke_with_the_model_count() {
        let inner = Arc::new(MockModel::new());
        inner
            .add_turn_with(
                vec!["First response".into()],
                None,
                Some(Usage::new(200, 50, 250)),
            )
            .add_turn(vec!["Second response".into()]);
        let mut agent = agent(counting(&inner, Some(15)), Vec::new(), Vec::new());
        agent.invoke("First question").await.unwrap();
        let seen = capture_projected(&agent);

        agent.invoke("Second question").await.unwrap();

        assert_eq!(*seen.lock().unwrap(), [Some(265)]);
    }
}

#[tokio::test]
async fn sends_an_injected_block_to_the_model_as_text_and_keeps_it_in_agent_messages() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec!["ok".into()]);
    let mut agent = agent(model.clone(), Vec::new(), Vec::new());
    let prompt = vec![
        ContentBlock::from("Refactor the tokenizer."),
        ContentBlock::Injected(InjectedBlock::new("reminder", "test")),
    ];

    agent.invoke(prompt.clone()).await.unwrap();

    assert_eq!(
        model.requests()[0].messages[0].content,
        [
            ContentBlock::from("Refactor the tokenizer."),
            ContentBlock::from("\n\nreminder")
        ]
    );
    assert_eq!(agent.messages()[0].content, prompt);
}

#[tokio::test]
async fn leaves_valid_names_untouched() {
    let model = Arc::new(MockModel::new());
    model.add_turn(vec!["ok".into()]);
    let tool_use = ToolUseBlock::new("good_tool-1", "tu-1", json!({}));
    let history = vec![
        Message::new(Role::User, vec!["do thing".into()]),
        Message::new(Role::Assistant, vec![ContentBlock::ToolUse(tool_use.clone())]),
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock::new(
                "tu-1",
                ToolResultStatus::Success,
                vec![ToolResultContent::from("result")],
            ))],
        ),
    ];
    let mut agent = agent(model.clone(), history, Vec::new());

    agent.invoke("continue").await.unwrap();

    let request = &model.requests()[0];
    let sent = request
        .messages
        .iter()
        .find(|message| message.role == Role::Assistant)
        .unwrap();
    assert_eq!(sent.content, [ContentBlock::ToolUse(tool_use)]);
}

mod redact_last_message {
    use super::*;

    const REDACT_MESSAGE: &str = "[REDACTED]";

    fn redacted(messages: Vec<Message>) -> Agent {
        let mut agent = agent(Arc::new(MockModel::new()), messages, Vec::new());
        agent.redact_last_message(REDACT_MESSAGE);
        agent
    }

    #[test]
    fn redacts_last_user_message_with_only_text_blocks() {
        let agent = redacted(vec![Message::new(Role::User, vec!["sensitive content".into()])]);
        let last = agent.messages().last().unwrap();
        assert_eq!(last.role, Role::User);
        assert_eq!(last.content, [ContentBlock::from(REDACT_MESSAGE)]);
    }

    #[test]
    fn preserves_the_durable_tracking_id_when_redacting() {
        let message =
            Message::new(Role::User, vec!["sensitive content".into()]).with_tracking_id("durable-1");
        let agent = redacted(vec![message]);
        assert_eq!(agent.messages().last().unwrap().tracking_id(), "durable-1");
    }

    #[test]
    fn preserves_tool_result_blocks_with_redacted_content() {
        let result = |id: &str, status: ToolResultStatus, text: &str| {
            ContentBlock::ToolResult(ToolResultBlock::new(
                id,
                status,
                vec![ToolResultContent::from(text)],
            ))
        };
        let agent = redacted(vec![Message::new(
            Role::User,
            vec![
                "some text".into(),
                result("tool-1", ToolResultStatus::Success, "tool result content"),
                "more text".into(),
                result("tool-2", ToolResultStatus::Error, "error content"),
            ],
        )]);
        let last = agent.messages().last().unwrap();
        assert_eq!(last.role, Role::User);
        assert_eq!(
            last.content,
            [
                result("tool-1", ToolResultStatus::Success, REDACT_MESSAGE),
                result("tool-2", ToolResultStatus::Error, REDACT_MESSAGE),
            ]
        );
    }

    #[test]
    fn does_not_redact_when_last_message_is_not_from_user() {
        let message = Message::new(Role::Assistant, vec!["assistant response".into()]);
        let agent = redacted(vec![message.clone()]);
        assert_eq!(agent.messages(), [message]);
    }

    #[test]
    fn handles_empty_messages_array_gracefully() {
        let agent = redacted(Vec::new());
        assert!(agent.messages().is_empty());
    }
}
