//! Live context-management tests against Bedrock (TS checklist:
//! strands-ts/test/integ/conversation-manager/agentic-context.test.ts,
//! context-manager-message-reduction.test.ts, context-manager-summarize-tool-calls.test.ts and
//! summarizing-conversation-manager.test.ts). The TS cases are grouped into a few agent runs;
//! each test's doc comment names the TS cases it covers.
//!
//! TS calls `manager.reduce(..)` and `strategy.apply(..)` directly. Their Rust arguments borrow
//! the agent's loop-side state, which only the agent loop hands out, so these tests reach the
//! same code through an invocation: a recording model wrapper fails the first call with a
//! context-window overflow (the conversation manager cases) or reports a small context window
//! (the strategy cases).
//!
//! They use the default AWS credential chain and skip with a message when no credentials
//! resolve. Run with `cargo test -p strands --all-features -- --ignored live_`.
#![cfg(feature = "bedrock")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    reason = "test code"
)]

mod live_support;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use futures::stream::BoxStream;
use live_support::{
    REGION, bedrock, credentials_available, last_assistant_text, tool_attempted, tool_result_contains,
    tool_results, tool_succeeded, tool_uses, trace,
};
use serde_json::{Value, json};
use strands::conversation_manager::{SummarizingConversationManager, SummarizingConversationManagerConfig};
use strands::error::ContextWindowOverflowError;
use strands::experimental::context_manager::{
    ContextManagerConfig, ContextManagerPreset, ContextStrategyEntry, Offload, OffloadConditions,
    OffloadTarget, SummarizeConfig, TruncateConfig,
};
use strands::models::bedrock::{BedrockModel, BedrockModelConfig, BedrockModelOptions};
use strands::models::model::{BaseModelConfig, CountTokensOptions, Model, ModelRequest};
use strands::models::model_info::ModelInfo;
use strands::models::streaming::ModelStreamEvent;
use strands::types::messages::{MessageMetadata, Role, TextBlock, ToolResultBlock, ToolUseBlock};
use strands::types::tools::ToolResultStatus;
use strands::{Agent, Message, StopReason, ToolError, tool};

/// Records every request to the wrapped model (TS `vi.spyOn(model, 'stream')`), and can fail the
/// first one with a context-window overflow.
struct Spy {
    inner: BedrockModel,
    requests: Mutex<Vec<Vec<Value>>>,
    overflow_once: AtomicBool,
}

impl Spy {
    fn new(inner: BedrockModel) -> Arc<Self> {
        Arc::new(Self {
            inner,
            requests: Mutex::new(Vec::new()),
            overflow_once: AtomicBool::new(false),
        })
    }

    fn overflowing_once(inner: BedrockModel) -> Arc<Self> {
        let spy = Self::new(inner);
        spy.overflow_once.store(true, Ordering::SeqCst);
        spy
    }

    /// The messages of each request, in their wire shape.
    fn requests(&self) -> Vec<Vec<Value>> {
        self.requests.lock().unwrap().clone()
    }

    fn calls(&self) -> usize {
        self.requests.lock().unwrap().len()
    }
}

#[strands::async_trait]
impl Model for Spy {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, strands::Result<ModelStreamEvent>> {
        let messages = request
            .messages
            .iter()
            .map(|message| serde_json::to_value(message).unwrap())
            .collect();
        self.requests.lock().unwrap().push(messages);
        if self.overflow_once.swap(false, Ordering::SeqCst) {
            return Box::pin(futures::stream::once(async {
                Err(ContextWindowOverflowError::new("overflow").into())
            }));
        }
        Model::stream(&self.inner, request)
    }

    fn get_config(&self) -> BaseModelConfig {
        Model::get_config(&self.inner)
    }

    fn update_config(&self, config: BaseModelConfig) {
        Model::update_config(&self.inner, config);
    }

    fn model_info(&self) -> ModelInfo {
        Model::model_info(&self.inner)
    }

    async fn count_tokens(&self, messages: &[Message], options: CountTokensOptions) -> strands::Result<u64> {
        Model::count_tokens(&self.inner, messages, options).await
    }
}

/// Add two numbers.
#[tool(name = "calculator")]
async fn add(a: f64, b: f64) -> Result<String, ToolError> {
    Ok(format!("{}", a + b))
}

/// Performs basic arithmetic operations.
#[tool(name = "calculator")]
async fn arithmetic(
    /// One of "add", "subtract", "multiply" or "divide".
    operation: String,
    a: f64,
    b: f64,
) -> Result<String, ToolError> {
    let value = match operation.as_str() {
        "add" => a + b,
        "subtract" => a - b,
        "multiply" => a * b,
        "divide" => a / b,
        other => return Err(ToolError::msg(format!("unknown operation {other}"))),
    };
    Ok(format!("Result: {value}"))
}

fn text(role: Role, text: &str) -> Message {
    Message::new(role, vec![TextBlock::new(text).into()])
}

fn tool_use(name: &str, id: &str, input: Value) -> Message {
    Message::new(Role::Assistant, vec![ToolUseBlock::new(name, id, input).into()])
}

fn tool_result(id: &str, text: &str) -> Message {
    Message::new(
        Role::User,
        vec![ToolResultBlock::new(id, ToolResultStatus::Success, vec![text.into()]).into()],
    )
}

/// The `<context-status>` note of the last message of a request, if it has one.
fn context_status(request: &[Value]) -> Option<String> {
    request.last()?["content"]
        .as_array()?
        .iter()
        .filter_map(|block| block["text"].as_str())
        .find(|text| text.contains("<context-status>"))
        .map(str::to_owned)
}

fn has_user_text(messages: &[Value], needle: &str) -> bool {
    messages.iter().any(|message| {
        message["content"]
            .as_array()
            .unwrap()
            .iter()
            .any(|block| block["text"].as_str().is_some_and(|text| text.contains(needle)))
    })
}

fn last_input_tokens(messages: &[Value]) -> u64 {
    messages
        .iter()
        .rev()
        .find_map(|message| message["metadata"]["usage"]["inputTokens"].as_u64())
        .unwrap_or(0)
}

/// TS cases (agentic-context): `agent responds normally with contextManager agentic enabled`,
/// `works alongside user-defined tools`, `context-status XML is present in messages sent to
/// model`, `context-status contains token usage and remaining capacity`, `context-status is NOT
/// injected when contextManager is not agentic`, `token usage in message metadata grows across
/// turns`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_agentic_context_status_is_injected_and_token_usage_grows_across_turns() {
    if !credentials_available().await {
        return;
    }
    let spy = Spy::new(bedrock(1024));
    let mut agent = Agent::builder()
        .model(Arc::clone(&spy) as Arc<dyn Model>)
        .context_manager(ContextManagerPreset::Agentic)
        .tools([Add])
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke("Use the calculator tool to add 17 and 25.")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let messages = trace(&agent);
    assert!(tool_succeeded(&messages, "calculator"));
    assert!(tool_result_contains(&messages, "calculator", "42"));
    let status = spy
        .requests()
        .iter()
        .find_map(|request| context_status(request))
        .expect("a request ends with the context status");
    assert!(
        status.contains("<used>") && status.contains("<remaining>") && status.contains("</context-status>")
    );
    // The note is sent to the model, not stored in the history.
    assert!(!has_user_text(&messages, "<context-status>"));

    let first = last_input_tokens(&messages);
    agent.invoke("Now count from 1 to 10.").await.unwrap();
    let second = last_input_tokens(&trace(&agent));
    assert!(first > 0 && second > first);
    agent.shutdown().await.unwrap();

    let spy = Spy::new(bedrock(200));
    let mut plain = Agent::builder()
        .model(Arc::clone(&spy) as Arc<dyn Model>)
        .printer(false)
        .build()
        .unwrap();
    plain.invoke("Say hi.").await.unwrap();
    assert_eq!(spy.calls(), 1);
    assert!(
        spy.requests()
            .iter()
            .all(|request| context_status(request).is_none())
    );
    plain.shutdown().await.unwrap();
}

/// TS cases (agentic-context): `truncate_context actually drops messages`, `pin protects
/// messages from truncation`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_agentic_truncate_context_drops_messages_and_pin_protects_them() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(2048))
        .context_manager(ContextManagerPreset::Agentic)
        .system_prompt(
            "When asked to pin, use the pin tool. When asked to truncate, use truncate_context. \
             Keep responses to 1 sentence.",
        )
        .printer(false)
        .build()
        .unwrap();
    agent
        .invoke("IMPORTANT: The secret code is ZEBRA-9.")
        .await
        .unwrap();
    agent
        .invoke("Pin the first message using indices [0].")
        .await
        .unwrap();
    agent.invoke("Some filler conversation.").await.unwrap();
    agent.invoke("More filler conversation.").await.unwrap();
    let before = agent.messages().len();
    assert!(
        tool_succeeded(&trace(&agent), "pin_context"),
        "{:#?}",
        trace(&agent)
    );
    agent
        .invoke("Truncate context, keep only recent 2 messages.")
        .await
        .unwrap();
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "truncate_context"));
    assert!(messages.len() < before);
    assert!(has_user_text(&messages, "ZEBRA-9"));
    assert!(!has_user_text(&messages, "Some filler conversation."));
    agent.shutdown().await.unwrap();
}

/// TS cases (agentic-context): `model uses summarize_context when explicitly asked`,
/// `summarize_context actually replaces older messages with a summary`, `agent remains coherent
/// after summarization`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_agentic_summarize_context_replaces_older_messages_and_agent_remains_coherent() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(bedrock(2048))
        .context_manager(ContextManagerPreset::Agentic)
        .system_prompt(
            "When asked to compress, call summarize_context with keepRecent=2 and summaryRatio=0.6, then stop. \
             Keep responses to 1 sentence.",
        )
        .printer(false)
        .build()
        .unwrap();
    for fact in [
        "My name is Alice and I live in Portland.",
        "Grass is green.",
        "I have a dog named Biscuit.",
        "Fire is hot.",
        "Ice is cold.",
        "Snow is white.",
    ] {
        agent.invoke(fact).await.unwrap();
    }
    let before = agent.messages().len();
    agent
        .invoke("Compress context using summarize_context with keepRecent=2 and summaryRatio=0.6.")
        .await
        .unwrap();
    let messages = trace(&agent);
    assert!(tool_attempted(&messages, "summarize_context"));
    assert!(messages.len() < before);
    let kept_verbatim = messages
        .iter()
        .any(|message| message["role"] == "user" && message["content"][0]["text"] == "Grass is green.");
    assert!(!kept_verbatim);

    let result = agent.invoke("What is my name?").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(last_assistant_text(&trace(&agent)).contains("alice"));
    agent.shutdown().await.unwrap();
}

const TASK: &str = "You help with arithmetic. Always use the calculator tool.";

fn pinned(message: Message) -> Message {
    let custom = serde_json::Map::from_iter([("pinned".to_owned(), json!(true))]);
    message.with_metadata(MessageMetadata::new().with_custom(custom))
}

fn reduction_strategy(name: &str) -> ContextStrategyEntry {
    let at_half = OffloadConditions {
        utilization: Some(0.5),
        ..Default::default()
    };
    match name {
        "drop" => Offload::drop(OffloadTarget::All)
            .unwrap()
            .when(at_half)
            .unwrap()
            .into(),
        "truncate" => Offload::truncate(OffloadTarget::All, TruncateConfig::default())
            .unwrap()
            .when(at_half)
            .unwrap()
            .into(),
        _ => Offload::summarize(OffloadTarget::All, SummarizeConfig::default())
            .unwrap()
            .when(OffloadConditions {
                preserve_recent: Some(2.0),
                ..at_half
            })
            .unwrap()
            .into(),
    }
}

/// TS cases (context-manager-message-reduction): `%s keeps message 0 verbatim and every reduced
/// history valid` for drop, truncate and summarize, on the default and the thinking model.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_drop_truncate_and_summarize_keep_message_0_verbatim_and_every_reduced_history_valid() {
    if !credentials_available().await {
        return;
    }
    let thinking = json!({"thinking": {"type": "enabled", "budget_tokens": 1024}});
    let models = [
        (live_support::MODEL_ID, None),
        ("global.anthropic.claude-sonnet-4-6", Some(thinking)),
    ];
    for (model_id, additional_request_fields) in models {
        for strategy in ["drop", "truncate", "summarize"] {
            let model = BedrockModel::new(BedrockModelOptions {
                config: BedrockModelConfig {
                    model_id: Some(model_id.into()),
                    max_tokens: Some(2048),
                    context_window_limit: Some(400),
                    additional_request_fields: additional_request_fields.clone(),
                    ..Default::default()
                },
                region: Some(REGION.into()),
                ..Default::default()
            });
            let mut agent = Agent::builder()
                .model(model)
                .context_manager(ContextManagerConfig {
                    strategies: Some(vec![reduction_strategy(strategy)]),
                    ..Default::default()
                })
                .tools([Add])
                .messages(vec![
                    pinned(text(Role::User, TASK)),
                    text(Role::Assistant, "Understood."),
                ])
                .printer(false)
                .build()
                .unwrap();
            for index in 0..4 {
                let prompt = format!("Use the calculator to add {index} and {}.", index + 10);
                let result = agent
                    .invoke(prompt)
                    .await
                    .unwrap_or_else(|error| panic!("{strategy} on {model_id}, turn {index}: {error}"));
                assert_eq!(
                    result.stop_reason(),
                    StopReason::EndTurn,
                    "{strategy} on {model_id}"
                );
            }
            let messages = trace(&agent);
            assert_eq!(messages[0]["role"], "user", "{strategy} on {model_id}");
            assert_eq!(
                messages[0]["content"],
                json!([{ "text": TASK }]),
                "{strategy} on {model_id}"
            );
            assert!(
                messages.len() < 10,
                "{strategy} on {model_id}: {} messages",
                messages.len()
            );
            agent.shutdown().await.unwrap();
        }
    }
}

/// TS case (context-manager-summarize-tool-calls, Bedrock): `summarizes the tool call in a
/// single summarizer request`. The strategy runs before the model call of an invocation; the
/// summarizer is its own recording model, so its request count is exact.
///
/// The last assertion (the summary names the tracking number, as TS asserts) depends on the
/// model's wording and failed in 2 of 35 runs on 2026-11-16: the request held the tool call and
/// its result, but the model summarized the `lookup_order` call as one it should not have made
/// (the request carries only the `noop` tool spec, as in TS) and left the result out.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_summarizes_the_tool_call_in_a_single_summarizer_request() {
    if !credentials_available().await {
        return;
    }
    let summarizer = Spy::new(bedrock(1024));
    let strategy = Offload::summarize(
        OffloadTarget::All,
        SummarizeConfig {
            model: Some(Arc::clone(&summarizer) as Arc<dyn Model>),
            ..Default::default()
        },
    )
    .unwrap()
    .when(OffloadConditions {
        utilization: Some(0.5),
        preserve_recent: Some(2.0),
        ..Default::default()
    })
    .unwrap();
    let model = BedrockModel::new(BedrockModelOptions {
        config: BedrockModelConfig {
            model_id: Some(live_support::MODEL_ID.into()),
            max_tokens: Some(300),
            context_window_limit: Some(60),
            ..Default::default()
        },
        region: Some(REGION.into()),
        ..Default::default()
    });
    let mut agent = Agent::builder()
        .model(model)
        .context_manager(ContextManagerConfig {
            strategies: Some(vec![strategy.into()]),
            ..Default::default()
        })
        .messages(vec![
            text(Role::User, "Where is my order?"),
            tool_use("lookup_order", "call_1", json!({"orderId": "ORD-48213"})),
            tool_result(
                "call_1",
                "Shipped via FedEx, tracking FX-7731, arriving Thursday.",
            ),
            text(Role::Assistant, "Your order shipped and arrives Thursday."),
        ])
        .printer(false)
        .build()
        .unwrap();
    let result = agent
        .invoke("Thanks. Can I change the delivery address?")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(summarizer.calls(), 1);
    let messages = trace(&agent);
    let summaries: Vec<&str> = messages
        .iter()
        .filter_map(|message| message["content"].as_array())
        .flatten()
        .filter_map(|block| block["text"].as_str())
        .filter(|text| text.starts_with("[Summarized: "))
        .collect();
    assert_eq!(summaries.len(), 1, "{summaries:?}");
    // The summarizer was sent the tool call and its result, and the summary covers those two messages.
    let sent = serde_json::to_string(&summarizer.requests()[0]).unwrap();
    assert!(
        sent.contains("FX-7731") && sent.contains("lookup_order"),
        "{sent}"
    );
    assert!(
        summaries[0].starts_with("[Summarized: 2 messages"),
        "{}",
        summaries[0]
    );
    assert!(
        summaries[0].contains("FX-7731"),
        "summary: {}\nsent: {sent}",
        summaries[0]
    );
    assert!(tool_uses(&messages).is_empty() && tool_results(&messages).is_empty());
    agent.shutdown().await.unwrap();
}

/// TS case (summarizing-conversation-manager): `summarizes older messages and agent remains
/// functional after summarization`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_summarizes_older_messages_and_agent_remains_functional_after_summarization() {
    if !credentials_available().await {
        return;
    }
    let history = vec![
        text(Role::User, "Hello, I am testing a conversation manager."),
        text(
            Role::Assistant,
            "Hello! I am here to help you test the conversation manager.",
        ),
        text(Role::User, "Can you tell me about the history of computers?"),
        text(
            Role::Assistant,
            "The history of computers spans many centuries, from the abacus to modern machines. Key milestones \
             include the Pascaline (1642), ENIAC (1945), the transistor (1947) and the microprocessor (1971).",
        ),
        text(Role::User, "What were the first computers like?"),
        text(
            Role::Assistant,
            "Early computers like ENIAC were enormous room-filling machines weighing about 30 tons, using \
             thousands of vacuum tubes that generated tremendous heat.",
        ),
    ];
    let spy = Spy::overflowing_once(bedrock(1024));
    let manager = SummarizingConversationManager::new(
        SummarizingConversationManagerConfig::builder()
            .summary_ratio(0.5)
            .preserve_recent_messages(2)
            .build(),
    )
    .unwrap();
    let mut agent = Agent::builder()
        .model(Arc::clone(&spy) as Arc<dyn Model>)
        .conversation_manager(manager)
        .messages(history)
        .printer(false)
        .build()
        .unwrap();
    let result = agent.invoke("Thanks for the overview!").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    // The overflowing call, the summarizer's request, then the retried call.
    assert_eq!(spy.calls(), 3);
    let messages = trace(&agent);
    assert!(messages.len() < 8, "{} messages", messages.len());
    assert_eq!(messages[0]["role"], "user");
    assert!(messages[0]["content"][0]["text"].as_str().unwrap().len() > 50);
    assert!(!has_user_text(
        &messages,
        "Hello, I am testing a conversation manager."
    ));
    assert!(has_user_text(&messages, "Thanks for the overview!"));
    assert_eq!(messages.last().unwrap()["role"], "assistant");
    // The retried request is the reduced history.
    assert!(spy.requests()[2].len() < spy.requests()[0].len());
    agent.shutdown().await.unwrap();
}

/// TS case (summarizing-conversation-manager): `keeps tool use/result pairs balanced after
/// summarization`.
#[tokio::test]
#[ignore = "live bedrock"]
async fn live_keeps_tool_use_result_pairs_balanced_after_summarization() {
    if !credentials_available().await {
        return;
    }
    let history = vec![
        text(Role::User, "Hello, can you help me with some calculations?"),
        text(Role::Assistant, "Of course! I can help with calculations."),
        text(Role::User, "What is the current time?"),
        tool_use("get_time", "time-1", json!({})),
        tool_result("time-1", "2024-01-15 14:30:00"),
        text(Role::Assistant, "The current time is 2024-01-15 14:30:00."),
        text(Role::User, "What is 25 + 37?"),
        tool_use(
            "calculator",
            "calc-1",
            json!({"operation": "add", "a": 25, "b": 37}),
        ),
        tool_result("calc-1", "62"),
        text(Role::Assistant, "25 + 37 = 62"),
        text(Role::User, "What is the weather in San Francisco?"),
        tool_use("get_weather", "weather-1", json!({"city": "San Francisco"})),
        tool_result("weather-1", "Sunny and 72°F in San Francisco"),
        text(Role::Assistant, "The weather in San Francisco is sunny and 72°F."),
    ];
    let spy = Spy::overflowing_once(bedrock(1024));
    let manager = SummarizingConversationManager::new(
        SummarizingConversationManagerConfig::builder()
            .summary_ratio(0.6)
            .preserve_recent_messages(3)
            .build(),
    )
    .unwrap();
    let mut agent = Agent::builder()
        .model(Arc::clone(&spy) as Arc<dyn Model>)
        .conversation_manager(manager)
        .tools([Arithmetic])
        .messages(history)
        .printer(false)
        .build()
        .unwrap();
    let result = agent.invoke("Calculate 15 + 28 for me.").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let reduced = &spy.requests()[2];
    assert!(reduced.len() < spy.requests()[0].len());
    // Every tool result of the reduced history answers a tool use that is still in it.
    let use_ids: Vec<&Value> = tool_uses(reduced)
        .iter()
        .map(|block| &block["toolUseId"])
        .collect();
    let result_ids: Vec<&Value> = tool_results(reduced)
        .iter()
        .map(|block| &block["toolUseId"])
        .collect();
    assert_eq!(use_ids, result_ids);
    assert!(!use_ids.contains(&&json!("time-1")));
    // The agent still works with tools.
    let messages = trace(&agent);
    assert!(tool_result_contains(&messages, "calculator", "Result: 43"));
    agent.shutdown().await.unwrap();
}
