//! Live tests against Amazon Bedrock Mantle through `OpenAIModel`
//! (strands-ts/test/integ/models/openai/mantle.test.node.ts is the checklist). They use the
//! default AWS credential chain and skip with a message when it resolves no credentials.

#![cfg(all(feature = "openai", feature = "bedrock"))]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test code; the skip message goes to stderr"
)]

use aws_credential_types::provider::ProvideCredentials as _;
use futures::StreamExt;
use serde_json::json;
use strands::models::model::{CacheConfig, Model, ModelRequest};
use strands::models::openai::{
    BedrockMantleConfig, OpenAIApi, OpenAIModel, OpenAIModelConfig, OpenAIModelOptions,
};
use strands::models::streaming::ModelStreamEvent;
use strands::types::messages::{ContentBlock, Message, Role, StopReason, TextBlock};
use strands::{Agent, InvokeOptions, ToolError, tool};

const REGION: &str = "us-east-1";
const MODEL_ID: &str = "openai.gpt-oss-120b";
/// The Mantle model id the shared model info gives `promptCaching: cachePoints`.
const CACHE_MODEL_ID: &str = "openai.gpt-6.1-sol";

async fn has_credentials() -> bool {
    let config = aws_config::defaults(aws_config::BehaviorVersion::latest())
        .region(aws_config::Region::new(REGION))
        .load()
        .await;
    let resolved = match config.credentials_provider() {
        Some(provider) => provider.provide_credentials().await.is_ok(),
        None => false,
    };
    if !resolved {
        eprintln!("skipping: no AWS credentials in the default chain");
    }
    resolved
}

async fn model(api: OpenAIApi, config: OpenAIModelConfig) -> Option<OpenAIModel> {
    if !has_credentials().await {
        return None;
    }
    let options = OpenAIModelOptions {
        api,
        config: OpenAIModelConfig {
            model_id: config.model_id.or_else(|| Some(MODEL_ID.to_owned())),
            ..config
        },
        ..Default::default()
    };
    let mantle = BedrockMantleConfig {
        region: Some(REGION.to_owned()),
        ..Default::default()
    };
    Some(OpenAIModel::with_bedrock_mantle(options, mantle).expect("the model builds"))
}

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

async fn events(model: &OpenAIModel, request: ModelRequest) -> Vec<ModelStreamEvent> {
    let mut stream = model.stream(request);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("the stream succeeds"));
    }
    events
}

fn usage_of(events: &[ModelStreamEvent]) -> strands::models::streaming::Usage {
    events
        .iter()
        .find_map(|event| match event {
            ModelStreamEvent::Metadata(metadata) => metadata.usage().cloned(),
            _ => None,
        })
        .expect("usage is reported")
}

fn reply_text(message: &Message) -> String {
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text().to_owned()),
            _ => None,
        })
        .collect()
}

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name.
    city: String,
) -> Result<String, ToolError> {
    Ok(format!("It is 21 degrees and sunny in {city}."))
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Person {
    name: String,
    age: u32,
}

async fn text_turn(api: OpenAIApi) {
    let Some(model) = model(api, OpenAIModelConfig::default()).await else {
        return;
    };
    let mut agent = Agent::builder()
        .model(model)
        .system_prompt("Reply in one short sentence.")
        .printer(false)
        .build()
        .unwrap();
    let result = agent.invoke("What is 2+2?").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(reply_text(result.last_message()).contains('4'));
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_text_chat() {
    text_turn(OpenAIApi::Chat).await;
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_text_responses() {
    text_turn(OpenAIApi::Responses).await;
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_streaming() {
    let Some(model) = model(OpenAIApi::Responses, OpenAIModelConfig::default()).await else {
        return;
    };
    let request = ModelRequest {
        messages: vec![user("Count from 1 to 5.")],
        ..Default::default()
    };
    let events = events(&model, request).await;
    assert!(matches!(events.first(), Some(ModelStreamEvent::MessageStart(_))));
    assert!(events.iter().any(|event| event.text_delta().is_some()));
    let usage = usage_of(&events);
    assert!(
        usage.input_tokens() > 0 && usage.output_tokens() > 0,
        "{}",
        json!(usage)
    );
}

async fn tool_round_trip(api: OpenAIApi) {
    let Some(model) = model(api, OpenAIModelConfig::default()).await else {
        return;
    };
    let mut agent = Agent::builder()
        .model(model)
        .printer(false)
        .tools([GetWeather])
        .build()
        .unwrap();
    let result = agent
        .invoke("Use the get_weather tool for Lisbon, then answer.")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let blocks: Vec<&ContentBlock> = agent.messages().iter().flat_map(Message::content).collect();
    let tool_use = blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolUse(tool_use) => Some(tool_use),
            _ => None,
        })
        .expect("the model called a tool");
    assert_eq!(tool_use.name(), "get_weather");
    assert!(tool_use.input()["city"].as_str().unwrap().contains("Lisbon"));
    let tool_result = blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(serde_json::to_string(result).unwrap()),
            _ => None,
        })
        .expect("the tool ran");
    assert!(tool_result.contains("21 degrees"));
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_tool_round_trip_chat() {
    tool_round_trip(OpenAIApi::Chat).await;
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_tool_round_trip_responses() {
    tool_round_trip(OpenAIApi::Responses).await;
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_structured_output() {
    let Some(model) = model(OpenAIApi::Chat, OpenAIModelConfig::default()).await else {
        return;
    };
    let mut agent = Agent::builder().model(model).printer(false).build().unwrap();
    let person: Person = agent
        .invoke_with(
            "John Smith is 30 years old.",
            InvokeOptions::default().structured_output_schema::<Person>(),
        )
        .await
        .unwrap()
        .structured_output()
        .unwrap();
    assert!(person.name.contains("John"));
    assert_eq!(person.age, 30);
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_stateful_conversation() {
    let config = OpenAIModelConfig {
        stateful: Some(true),
        ..Default::default()
    };
    let Some(model) = model(OpenAIApi::Responses, config).await else {
        return;
    };
    let mut agent = Agent::builder()
        .model(model)
        .system_prompt("Reply in one short sentence.")
        .printer(false)
        .build()
        .unwrap();
    let first = agent.invoke("My name is Alice.").await.unwrap();
    assert_eq!(first.stop_reason(), StopReason::EndTurn);
    assert!(
        agent.messages().is_empty(),
        "a stateful model keeps no local history"
    );
    let result = agent.invoke("What is my name?").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(
        agent.messages().is_empty(),
        "a stateful model keeps no local history"
    );
    assert!(reply_text(result.last_message()).to_lowercase().contains("alice"));
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_reasoning_multi_turn() {
    let mut params = serde_json::Map::new();
    params.insert("reasoning".into(), json!({ "effort": "low" }));
    let config = OpenAIModelConfig {
        params: Some(params),
        ..Default::default()
    };
    let Some(model) = model(OpenAIApi::Responses, config).await else {
        return;
    };
    let mut agent = Agent::builder()
        .model(model)
        .system_prompt("Reply in one short sentence.")
        .printer(false)
        .build()
        .unwrap();
    let first = agent.invoke("What is 2+2?").await.unwrap();
    assert!(reply_text(first.last_message()).contains('4'));
    let second = agent.invoke("What about 3+3?").await.unwrap();
    assert_eq!(second.stop_reason(), StopReason::EndTurn);
}

#[tokio::test]
#[ignore = "live mantle"]
async fn live_mantle_cache_hit() {
    let config = OpenAIModelConfig {
        model_id: Some(CACHE_MODEL_ID.to_owned()),
        cache_config: Some(CacheConfig::default()),
        ..Default::default()
    };
    let Some(model) = model(OpenAIApi::Chat, config).await else {
        return;
    };
    let context = format!("Context information: {}", "repeat ".repeat(3000));
    let request = || ModelRequest {
        messages: vec![user(&context)],
        system_prompt: Some("You are a helpful assistant. Reply with one word.".into()),
        agent_metadata: Some(strands::agent::AgentMetadata {
            session_id: Some("rust-live-cache".into()),
        }),
        ..Default::default()
    };
    events(&model, request()).await;
    let second = usage_of(&events(&model, request()).await);
    assert!(
        second.cache_read_input_tokens().unwrap_or(0) > 0,
        "{}",
        json!(second)
    );
}
