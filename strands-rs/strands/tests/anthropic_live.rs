//! Live tests against the Anthropic Messages API (strands-ts/test/integ/models/anthropic.test.ts
//! is the checklist). They skip with a message when `ANTHROPIC_API_KEY` is not set.

#![cfg(feature = "anthropic")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test code; the skip message goes to stderr"
)]

use futures::StreamExt;
use serde_json::json;
use strands::models::anthropic::{AnthropicModel, AnthropicModelConfig};
use strands::models::model::{Model, ModelRequest};
use strands::models::streaming::ModelStreamEvent;
use strands::types::messages::{
    CachePointBlock, ContentBlock, Message, Role, StopReason, SystemContentBlock, SystemPrompt, TextBlock,
};
use strands::{Agent, InvokeOptions, ToolError, tool};

const MODEL_ID: &str = "claude-haiku-4-5";

fn model(max_tokens: u64) -> Option<AnthropicModel> {
    if std::env::var("ANTHROPIC_API_KEY").map_or(true, |key| key.trim().is_empty()) {
        eprintln!("skipping: ANTHROPIC_API_KEY is not set");
        return None;
    }
    let config = AnthropicModelConfig {
        model_id: Some(MODEL_ID.to_owned()),
        max_tokens: Some(max_tokens),
        ..Default::default()
    };
    Some(AnthropicModel::new(config).expect("the model builds"))
}

fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

async fn events(model: &AnthropicModel, request: ModelRequest) -> Vec<ModelStreamEvent> {
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

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name.
    city: String,
) -> Result<String, ToolError> {
    Ok(format!("It is 21 degrees and sunny in {city}."))
}

#[tokio::test]
#[ignore = "live anthropic"]
async fn live_text() {
    let Some(model) = model(64) else { return };
    let mut agent = Agent::builder().model(model).printer(false).build().unwrap();
    let result = agent.invoke("Reply with one short sentence.").await.unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    let has_text = result
        .last_message()
        .content()
        .iter()
        .any(|block| matches!(block, ContentBlock::Text(text) if !text.text().is_empty()));
    assert!(has_text);
}

#[tokio::test]
#[ignore = "live anthropic"]
async fn live_streaming() {
    let Some(model) = model(20) else { return };
    let request = ModelRequest {
        messages: vec![user("Write a very long story about space exploration.")],
        ..Default::default()
    };
    let events = events(&model, request).await;
    assert!(matches!(events.first(), Some(ModelStreamEvent::MessageStart(_))));
    assert!(events.iter().any(|event| event.text_delta().is_some()));
    assert!(usage_of(&events).output_tokens() <= 20);
    let stop = events.iter().find_map(|event| match event {
        ModelStreamEvent::MessageStop(stop) => Some(stop.stop_reason().clone()),
        _ => None,
    });
    assert_eq!(stop, Some(StopReason::MaxTokens));
}

#[tokio::test]
#[ignore = "live anthropic"]
async fn live_tool_round_trip() {
    let Some(model) = model(512) else { return };
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Person {
    name: String,
    age: u32,
}

#[tokio::test]
#[ignore = "live anthropic"]
async fn live_structured_output() {
    let Some(model) = model(512) else { return };
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
#[ignore = "live anthropic"]
async fn live_cache_hit() {
    let Some(model) = model(32) else { return };
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let context = format!("Context information: {} [{nonce}]", "repeat ".repeat(8000));
    let system_prompt = SystemPrompt::Blocks(vec![
        SystemContentBlock::text("You are a helpful assistant."),
        SystemContentBlock::text(context),
        CachePointBlock::new().into(),
    ]);
    let request = || ModelRequest {
        messages: vec![user("Say hi.")],
        system_prompt: Some(system_prompt.clone()),
        ..Default::default()
    };
    let first = usage_of(&events(&model, request()).await);
    assert!(
        first.cache_write_input_tokens().unwrap_or(0) > 0,
        "{}",
        json!(first)
    );
    let second = usage_of(&events(&model, request()).await);
    assert!(
        second.cache_read_input_tokens().unwrap_or(0) > 0,
        "{}",
        json!(second)
    );
}
