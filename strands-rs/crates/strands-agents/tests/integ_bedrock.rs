//! Live Bedrock integration tests (port of `strands-ts/test/integ/models/bedrock.test.ts` and the
//! Bedrock rows of `test/integ/agent.test.ts`). They call the real service and are `#[ignore]`d;
//! run with:
//!
//! ```text
//! AWS_REGION=us-west-2 cargo test --all-features --test integ_bedrock -- --ignored
//! ```

#![cfg(feature = "bedrock")]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use futures::StreamExt;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;
use strands_agents::agent::AgentStreamEvent;
use strands_agents::context_manager::ContextManagerOption;
use strands_agents::models::{
    BedrockGuardrailConfig, BedrockGuardrailRedactionConfig, BedrockModel, BedrockModelConfig, CacheConfig,
    CacheStrategy, ContentBlockDelta, CountTokensOptions, GuardrailTraceMode, Model, ModelStreamEvent,
    StreamOptions,
};
use strands_agents::tools::{FunctionTool, ToolContext};
use strands_agents::types::{CachePointBlock, ToolSpec};
use strands_agents::{Agent, ContentBlock, Error, Message, Role, StopReason, SystemPrompt};

const DEFAULT_MODEL_ID: &str = "global.anthropic.claude-sonnet-4-6";
const CACHING_MODEL_ID: &str = "us.anthropic.claude-sonnet-4-5-20250929-v1:0";

fn config(model_id: &str, max_tokens: i32) -> BedrockModelConfig {
    BedrockModelConfig {
        model_id: Some(model_id.to_owned()),
        max_tokens: Some(max_tokens),
        ..Default::default()
    }
}

async fn model(max_tokens: i32) -> BedrockModel {
    BedrockModel::new(config(DEFAULT_MODEL_ID, max_tokens))
        .await
        .expect("bedrock client")
}

async fn collect(
    model: &BedrockModel,
    messages: &[Message],
    options: &StreamOptions,
) -> Vec<ModelStreamEvent> {
    let mut events = Vec::new();
    let mut stream = model.stream(messages, options);
    while let Some(event) = stream.next().await {
        events.push(event.expect("stream event"));
    }
    events
}

fn usage_of(events: &[ModelStreamEvent]) -> strands_agents::types::Usage {
    events
        .iter()
        .find_map(|e| match e {
            ModelStreamEvent::Metadata { usage: Some(u), .. } => Some(u.clone()),
            _ => None,
        })
        .expect("metadata event with usage")
}

fn large_context() -> String {
    format!(
        "Context information: {} [test-{}-{}]",
        "hello ".repeat(2000),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis(),
        uuid::Uuid::new_v4()
    )
}

// ---------------------------------------------------------------- Streaming / Configuration

#[tokio::test]
#[ignore = "live Bedrock"]
async fn respects_max_tokens_configuration() {
    let model = model(20).await;
    let messages = vec![Message::user("Write a long story about dragons.")];
    let events = collect(&model, &messages, &StreamOptions::default()).await;

    assert!(usage_of(&events).output_tokens <= 20);
    let stop = events.iter().find_map(|e| match e {
        ModelStreamEvent::MessageStop { stop_reason, .. } => Some(stop_reason.clone()),
        _ => None,
    });
    assert_eq!(stop, Some(StopReason::MaxTokens));
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn uses_system_prompt_cache_on_subsequent_requests() {
    let model = BedrockModel::new(config(CACHING_MODEL_ID, 100)).await.unwrap();
    let system_prompt = SystemPrompt::Blocks(vec![
        strands_agents::types::SystemContentBlock::Text("You are a helpful assistant.".into()),
        strands_agents::types::SystemContentBlock::Text(large_context()),
        strands_agents::types::SystemContentBlock::CachePoint(CachePointBlock::default()),
    ]);
    let options = StreamOptions {
        system_prompt: Some(system_prompt),
        ..Default::default()
    };

    let first = collect(&model, &[Message::user("Say hello")], &options).await;
    assert!(
        usage_of(&first).cache_write_input_tokens.unwrap_or(0) > 0,
        "first call should write cache"
    );

    let second = collect(&model, &[Message::user("Say goodbye")], &options).await;
    assert!(
        usage_of(&second).cache_read_input_tokens.unwrap_or(0) > 0,
        "second call should read cache"
    );
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn uses_message_cache_points_on_subsequent_requests() {
    let model = BedrockModel::new(config(CACHING_MODEL_ID, 100)).await.unwrap();
    let context = large_context();
    let messages = |text: &str| {
        vec![Message::new(
            Role::User,
            vec![
                ContentBlock::text(context.clone()),
                ContentBlock::CachePoint(CachePointBlock::default()),
                ContentBlock::text(text),
            ],
        )]
    };

    let first = collect(&model, &messages("Say hello"), &StreamOptions::default()).await;
    assert!(usage_of(&first).cache_write_input_tokens.unwrap_or(0) > 0);
    let second = collect(&model, &messages("Say goodbye"), &StreamOptions::default()).await;
    assert!(usage_of(&second).cache_read_input_tokens.unwrap_or(0) > 0);
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn uses_cache_config_to_automatically_inject_cache_points_in_tools_and_messages() {
    let model = BedrockModel::new(BedrockModelConfig {
        cache_config: Some(CacheConfig {
            strategy: Some(CacheStrategy::Auto),
            ..Default::default()
        }),
        ..config(CACHING_MODEL_ID, 100)
    })
    .await
    .unwrap();
    let tool_specs = vec![ToolSpec::new("lookup", "Look up information. ".repeat(100))
        .with_input_schema(json!({"type": "object", "properties": {"query": {"type": "string"}}}))];
    let options = StreamOptions {
        tool_specs,
        ..Default::default()
    };
    let messages = vec![Message::user(large_context())];

    let first = collect(&model, &messages, &options).await;
    assert!(usage_of(&first).cache_write_input_tokens.unwrap_or(0) > 0);
    let second = collect(&model, &messages, &options).await;
    assert!(usage_of(&second).cache_read_input_tokens.unwrap_or(0) > 0);
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn uses_cache_config_with_explicit_anthropic_strategy() {
    let model = BedrockModel::new(BedrockModelConfig {
        cache_config: Some(CacheConfig {
            strategy: Some(CacheStrategy::Anthropic),
            ..Default::default()
        }),
        ..config(CACHING_MODEL_ID, 100)
    })
    .await
    .unwrap();
    let messages = vec![Message::user(large_context())];

    let first = collect(&model, &messages, &StreamOptions::default()).await;
    assert!(usage_of(&first).cache_write_input_tokens.unwrap_or(0) > 0);
    let second = collect(&model, &messages, &StreamOptions::default()).await;
    assert!(usage_of(&second).cache_read_input_tokens.unwrap_or(0) > 0);
}

// ---------------------------------------------------------------- Error handling

#[tokio::test]
#[ignore = "live Bedrock"]
async fn handles_invalid_model_id_gracefully() {
    let model = BedrockModel::new(config("invalid-model-id-that-does-not-exist", 100))
        .await
        .unwrap();
    let messages = [Message::user("Hello")];
    let options = StreamOptions::default();
    let mut stream = model.stream(&messages, &options);
    let mut saw_error = false;
    while let Some(event) = stream.next().await {
        if let Err(err) = event {
            assert!(err.is_model_error(), "expected a model error, got {err:?}");
            saw_error = true;
            break;
        }
    }
    assert!(saw_error, "invalid model id must surface as an error");
}

// ---------------------------------------------------------------- Agent + conversation manager

#[tokio::test]
#[ignore = "live Bedrock"]
async fn throws_without_context_management_when_the_model_hits_max_tokens() {
    // Mirrors the TS test: `maxTokens: 50` plus a long request makes the model stop on
    // `max_tokens`, which the SDK surfaces as `Error::MaxTokens`; no context manager recovers it.
    let agent = Agent::builder()
        .model(model(50).await)
        .context_manager(ContextManagerOption::Off)
        .build()
        .unwrap();
    let long_prompt = format!(
        "Please write a very detailed explanation of {}",
        "many topics ".repeat(50)
    );
    let err = agent.invoke(long_prompt).await.unwrap_err();
    assert!(matches!(err, Error::MaxTokens { .. }), "{err:?}");
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn maps_an_oversized_prompt_to_context_window_overflow() {
    // ~1.6M characters is far past the 200k-token window of Claude Haiku 4.5; Bedrock rejects it
    // with a ValidationException the SDK must classify as `ContextWindowOverflow`.
    let model = BedrockModel::new(config("us.anthropic.claude-haiku-4-5-20251001-v1:0", 50))
        .await
        .unwrap();
    let agent = Agent::builder()
        .model(model)
        .context_manager(ContextManagerOption::Off)
        .no_retry_strategy()
        .build()
        .unwrap();
    let huge = "lorem ipsum dolor sit amet ".repeat(60_000);
    let err = agent.invoke(huge).await.unwrap_err();
    assert!(matches!(err, Error::ContextWindowOverflow(_)), "{err:?}");
}

// ---------------------------------------------------------------- Thinking + tools

#[tokio::test]
#[ignore = "live Bedrock"]
async fn handles_thinking_mode_with_tool_use() {
    let model = BedrockModel::new(BedrockModelConfig {
        additional_request_fields: Some(json!({"thinking": {"type": "enabled", "budget_tokens": 1024}})),
        ..config(DEFAULT_MODEL_ID, 2048)
    })
    .await
    .unwrap();
    let test_tool = FunctionTool::builder("testTool")
        .description("Test description")
        .input_schema(json!({"type": "object"}))
        .handler(|_input, _ctx: ToolContext| async move { Ok("result") });
    let agent = Agent::builder().model(model).tool(test_tool).build().unwrap();

    let result = agent
        .invoke("Use the testTool with the message \"Hello World\"")
        .await
        .unwrap();

    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.last_message.role, Role::Assistant);
    assert!(!result.last_message.content.is_empty());
    let messages = agent.messages();
    assert!(
        messages.iter().any(|m| m.tool_uses().next().is_some()),
        "tool use recorded"
    );
    assert!(
        messages.iter().any(|m| m.tool_results().next().is_some()),
        "tool result recorded"
    );
    assert!(
        messages
            .iter()
            .any(|m| m.content.iter().any(|b| matches!(b, ContentBlock::Reasoning(_)))),
        "reasoning block recorded"
    );
}

// ---------------------------------------------------------------- countTokens

#[tokio::test]
#[ignore = "live Bedrock"]
async fn count_tokens_for_messages_only_and_more_with_tools_and_system_prompt() {
    let model = BedrockModel::new(BedrockModelConfig {
        use_native_token_count: Some(true),
        ..config(DEFAULT_MODEL_ID, 100)
    })
    .await
    .unwrap();
    let messages = vec![Message::user("What is the capital of France? Explain in detail.")];
    let without = model
        .count_tokens(&messages, &CountTokensOptions::default())
        .await
        .unwrap();
    assert!(without > 0);

    let options = CountTokensOptions {
        system_prompt: Some("Be helpful.".into()),
        tool_specs: vec![
            ToolSpec::new("get_weather", "Get the current weather for a location")
                .with_input_schema(json!({"type": "object", "properties": {"location": {"type": "string"}}})),
        ],
    };
    let with_tools = model.count_tokens(&messages, &options).await.unwrap();
    assert!(with_tools > without, "{with_tools} > {without}");
}

// ---------------------------------------------------------------- Agent basics (agent.test.ts)

#[derive(Deserialize, JsonSchema)]
struct CalcInput {
    a: f64,
    b: f64,
}

fn calculator(calls: Arc<AtomicUsize>) -> FunctionTool {
    FunctionTool::builder("calculator")
        .description("Multiply two numbers a and b and return the product")
        .typed_handler(move |input: CalcInput, _ctx: ToolContext| {
            let calls = calls.clone();
            async move {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(json!({ "result": input.a * input.b }))
            }
        })
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn handles_invocation_streaming_system_prompts_and_tool_use() {
    let calls = Arc::new(AtomicUsize::new(0));
    let agent = Agent::builder()
        .model(model(512).await)
        .system_prompt(
            "Use the calculator tool to solve math problems. Respond with only the numeric result.",
        )
        .tool(calculator(calls.clone()))
        .build()
        .unwrap();

    let mut stream = agent.stream("What is 123 * 456?");
    let mut saw_before_invocation = false;
    let mut result = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            AgentStreamEvent::BeforeInvocation(_) => saw_before_invocation = true,
            AgentStreamEvent::AgentResult(e) => result = Some(e.result),
            _ => {}
        }
    }
    let result = result.expect("result event");

    assert!(saw_before_invocation);
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert_eq!(result.last_message.role, Role::Assistant);
    assert!(calls.load(Ordering::SeqCst) >= 1, "calculator tool was called");
    assert!(agent.messages().iter().any(|m| m.tool_uses().next().is_some()));
    assert!(
        result.last_message.text().contains("56088"),
        "got: {}",
        result.last_message.text()
    );

    // Multi-turn after tool use keeps working.
    let follow_up = agent.invoke("What was the result?").await.unwrap();
    assert_eq!(follow_up.stop_reason, StopReason::EndTurn);
    assert!(agent.messages().len() >= 6);
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn handles_tool_use_with_json_schema_tool() {
    let tool = FunctionTool::builder("calculator")
        .description("Multiply two numbers a and b and return the product")
        .input_schema(json!({
            "type": "object",
            "properties": {"a": {"type": "number"}, "b": {"type": "number"}},
            "required": ["a", "b"]
        }))
        .handler(|input: serde_json::Value, _ctx: ToolContext| async move {
            let a = input["a"].as_f64().unwrap_or_default();
            let b = input["b"].as_f64().unwrap_or_default();
            Ok(json!({ "result": a * b }))
        });
    let agent = Agent::builder()
        .model(model(512).await)
        .system_prompt(
            "Use the calculator tool to solve math problems. Respond with only the numeric result.",
        )
        .tool(tool)
        .build()
        .unwrap();

    let result = agent.invoke("What is 25 * 48?").await.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert!(agent.messages().iter().any(|m| m.tool_uses().next().is_some()));
    assert!(
        result.last_message.text().contains("1200"),
        "got: {}",
        result.last_message.text()
    );
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn yields_metadata_events_through_the_agent_stream() {
    let agent = Agent::builder()
        .model(model(100).await)
        .system_prompt("Respond with a brief greeting.")
        .build()
        .unwrap();

    let mut stream = agent.stream("Say hello");
    let mut metadata = None;
    let mut result = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            AgentStreamEvent::ModelStreamUpdate(e) => {
                if let ModelStreamEvent::Metadata { usage, metrics, .. } = e.event {
                    metadata = Some((usage, metrics));
                }
            }
            AgentStreamEvent::AgentResult(e) => result = Some(e.result),
            _ => {}
        }
    }
    let (usage, metrics) = metadata.expect("metadata event");
    let usage = usage.expect("usage");
    assert!(usage.input_tokens > 0);
    assert!(usage.output_tokens > 0);
    assert!(metrics.expect("metrics").latency_ms > 0);
    let result = result.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert!(!result.last_message.content.is_empty());
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn maintains_message_history_and_conversation_context() {
    let agent = Agent::builder()
        .model(model(100).await)
        .system_prompt("Answer in one short sentence.")
        .build()
        .unwrap();
    agent
        .invoke("My favourite colour is teal. Remember it.")
        .await
        .unwrap();
    let result = agent.invoke("What is my favourite colour?").await.unwrap();
    assert_eq!(agent.messages().len(), 4);
    assert!(
        result.last_message.text().to_lowercase().contains("teal"),
        "got: {}",
        result.last_message.text()
    );
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn accepts_content_block_and_message_inputs() {
    let agent = Agent::builder().model(model(100).await).build().unwrap();
    let result = agent
        .invoke(vec![ContentBlock::text("Reply with the single word: pong")])
        .await
        .unwrap();
    assert!(result.last_message.text().to_lowercase().contains("pong"));

    let agent = Agent::builder().model(model(100).await).build().unwrap();
    let history = vec![
        Message::user("My name is Ada."),
        Message::assistant("Nice to meet you, Ada."),
        Message::user("What is my name? Answer with just the name."),
    ];
    let result = agent.invoke(history).await.unwrap();
    assert_eq!(agent.messages().len(), 4);
    assert!(
        result.last_message.text().contains("Ada"),
        "got: {}",
        result.last_message.text()
    );
}

#[derive(Debug, Deserialize, JsonSchema, PartialEq)]
struct Answer {
    answer: f64,
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn returns_validated_structured_output() {
    let agent = Agent::builder().model(model(200).await).build().unwrap();
    let answer: Answer = agent.structured_output("What is 2 + 2?").await.unwrap();
    assert_eq!(answer, Answer { answer: 4.0 });
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn emits_reasoning_content_with_thinking_model() {
    let model = BedrockModel::new(BedrockModelConfig {
        additional_request_fields: Some(json!({"thinking": {"type": "enabled", "budget_tokens": 1024}})),
        ..config(DEFAULT_MODEL_ID, 2048)
    })
    .await
    .unwrap();
    let agent = Agent::builder().model(model).build().unwrap();

    let mut stream = agent.stream("What is 15 * 23? Think step by step.");
    let mut reasoning_deltas = 0;
    let mut result = None;
    while let Some(event) = stream.next().await {
        match event.unwrap() {
            AgentStreamEvent::ModelStreamUpdate(e) => {
                if let ModelStreamEvent::ContentBlockDelta {
                    delta: ContentBlockDelta::ReasoningContent { .. },
                } = e.event
                {
                    reasoning_deltas += 1;
                }
            }
            AgentStreamEvent::AgentResult(e) => result = Some(e.result),
            _ => {}
        }
    }
    assert!(reasoning_deltas > 0, "expected reasoning deltas");
    let result = result.unwrap();
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert!(
        result.last_message.text().contains("345"),
        "got: {}",
        result.last_message.text()
    );
}

// ---------------------------------------------------------------- Non-streaming path

#[tokio::test]
#[ignore = "live Bedrock"]
async fn non_streaming_converse_produces_the_same_event_sequence() {
    let model = BedrockModel::new(BedrockModelConfig {
        stream: Some(false),
        ..config(DEFAULT_MODEL_ID, 50)
    })
    .await
    .unwrap();
    let events = collect(
        &model,
        &[Message::user("Reply with the single word: pong")],
        &StreamOptions::default(),
    )
    .await;
    let names: Vec<&str> = events.iter().map(|e| e.type_name()).collect();
    assert_eq!(names.first(), Some(&"modelMessageStartEvent"));
    assert!(names.contains(&"modelContentBlockDeltaEvent"));
    assert!(names.contains(&"modelMessageStopEvent"));
    assert!(names.contains(&"modelMetadataEvent"));
    assert!(usage_of(&events).output_tokens > 0);
}

#[tokio::test]
#[ignore = "live Bedrock"]
async fn stream_aggregated_assembles_a_message_with_usage_metadata() {
    let model = model(50).await;
    let messages = vec![Message::user("Reply with the single word: pong")];
    let options = StreamOptions::default();
    let mut stream = model.stream_aggregated(&messages, &options);
    let mut result = None;
    while let Some(item) = stream.next().await {
        if let strands_agents::models::AggregatedItem::Result(r) = item.unwrap() {
            result = Some(r);
        }
    }
    let result = result.expect("aggregated result");
    assert_eq!(result.stop_reason, StopReason::EndTurn);
    assert!(result.message.text().to_lowercase().contains("pong"));
    assert!(result
        .message
        .metadata
        .as_ref()
        .and_then(|m| m.usage.as_ref())
        .is_some());
}

// ---------------------------------------------------------------- Guardrails
//
// Needs a word-filter guardrail that blocks `CACTUS` with `BLOCKED_INPUT` / `BLOCKED_OUTPUT`
// messaging (TS creates `test-guardrail-block-cactus`). Point `STRANDS_TEST_GUARDRAIL_ID` at it;
// the test is skipped when the variable is unset. Create one with:
// `aws bedrock create-guardrail --name strandly-managed-rs-integ-cactus \
//    --word-policy-config '{"wordsConfig":[{"text":"CACTUS"}]}' \
//    --blocked-input-messaging BLOCKED_INPUT --blocked-outputs-messaging BLOCKED_OUTPUT \
//    --tags '[{"key":"ManagedBy","value":"strandly"}]'`

async fn guardrail_agent(trace: GuardrailTraceMode) -> Option<Agent> {
    let id = std::env::var("STRANDS_TEST_GUARDRAIL_ID").ok()?;
    let model = BedrockModel::new(BedrockModelConfig {
        guardrail_config: Some(BedrockGuardrailConfig {
            guardrail_identifier: id,
            guardrail_version: "DRAFT".into(),
            trace: Some(trace),
            redaction: Some(BedrockGuardrailRedactionConfig {
                input: Some(true),
                input_message: Some("Redacted.".into()),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..config(DEFAULT_MODEL_ID, 100)
    })
    .await
    .unwrap();
    Some(
        Agent::builder()
            .model(model)
            .system_prompt("You are a helpful assistant.")
            .build()
            .unwrap(),
    )
}

async fn assert_input_intervention(trace: GuardrailTraceMode) {
    let Some(agent) = guardrail_agent(trace).await else {
        eprintln!("STRANDS_TEST_GUARDRAIL_ID unset; skipping");
        return;
    };
    let blocked = agent.invoke("CACTUS").await.unwrap();
    let allowed = agent.invoke("Hello!").await.unwrap();

    assert_eq!(blocked.stop_reason, StopReason::GuardrailIntervened);
    assert_eq!(blocked.to_string().trim(), "BLOCKED_INPUT");
    assert_ne!(allowed.stop_reason, StopReason::GuardrailIntervened);
    assert_ne!(allowed.to_string().trim(), "BLOCKED_INPUT");
    let first = agent.messages().into_iter().next().unwrap();
    assert_eq!(
        first.content,
        vec![ContentBlock::text("Redacted.")],
        "blocked user input is redacted in history"
    );
}

#[tokio::test]
#[ignore = "live Bedrock + guardrail"]
async fn blocks_input_and_redacts_message_with_trace_disabled() {
    assert_input_intervention(GuardrailTraceMode::Disabled).await;
}

#[tokio::test]
#[ignore = "live Bedrock + guardrail"]
async fn blocks_input_and_redacts_message_with_trace_enabled() {
    assert_input_intervention(GuardrailTraceMode::Enabled).await;
}

#[tokio::test]
#[ignore = "live Bedrock + guardrail"]
async fn blocks_input_and_redacts_message_with_trace_enabled_full() {
    assert_input_intervention(GuardrailTraceMode::EnabledFull).await;
}
