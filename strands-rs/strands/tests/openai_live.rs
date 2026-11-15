//! Live tests against the OpenAI API (strands-ts/test/integ/models/openai/{responses,chat}.test.ts
//! are the checklist). They skip with a message when `OPENAI_API_KEY` is not set.

#![cfg(feature = "openai")]
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::print_stderr,
    reason = "test code; the skip message goes to stderr"
)]

use futures::StreamExt;
use serde_json::json;
use strands::models::model::{CacheConfig, Model, ModelRequest};
use strands::models::openai::{OpenAIApi, OpenAIModel, OpenAIModelConfig, OpenAIModelOptions};
use strands::models::streaming::ModelStreamEvent;
use strands::types::messages::{ContentBlock, Message, Role, StopReason, TextBlock};
use strands::{Agent, InvokeOptions, ToolError, tool};

const MODEL_ID: &str = "gpt-5.4-mini";

fn model(api: OpenAIApi, config: OpenAIModelConfig) -> Option<OpenAIModel> {
    if std::env::var("OPENAI_API_KEY").map_or(true, |key| key.trim().is_empty()) {
        eprintln!("skipping: OPENAI_API_KEY is not set");
        return None;
    }
    let options = OpenAIModelOptions {
        api,
        config: OpenAIModelConfig {
            model_id: Some(MODEL_ID.to_owned()),
            ..config
        },
        ..Default::default()
    };
    Some(OpenAIModel::new(options).expect("the model builds"))
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
            ModelStreamEvent::Metadata(metadata) => metadata.usage().copied(),
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

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Person {
    name: String,
    age: u32,
}

async fn text_turn(api: OpenAIApi) {
    let Some(model) = model(api, OpenAIModelConfig::default()) else {
        return;
    };
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
#[ignore = "live openai"]
async fn live_text_responses() {
    text_turn(OpenAIApi::Responses).await;
}

#[tokio::test]
#[ignore = "live openai"]
async fn live_text_chat() {
    text_turn(OpenAIApi::Chat).await;
}

#[tokio::test]
#[ignore = "live openai"]
async fn live_streaming() {
    let Some(model) = model(OpenAIApi::Responses, OpenAIModelConfig::default()) else {
        return;
    };
    let events = events(
        &model,
        ModelRequest {
            messages: vec![user("Count from 1 to 5.")],
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(events.first(), Some(ModelStreamEvent::MessageStart(_))));
    assert!(events.iter().any(|event| event.text_delta().is_some()));
    assert!(usage_of(&events).output_tokens() > 0);
}

async fn tool_round_trip(api: OpenAIApi) {
    let Some(model) = model(api, OpenAIModelConfig::default()) else {
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
#[ignore = "live openai"]
async fn live_tool_round_trip_responses() {
    tool_round_trip(OpenAIApi::Responses).await;
}

#[tokio::test]
#[ignore = "live openai"]
async fn live_tool_round_trip_chat() {
    tool_round_trip(OpenAIApi::Chat).await;
}

#[tokio::test]
#[ignore = "live openai"]
async fn live_structured_output() {
    let Some(model) = model(OpenAIApi::Responses, OpenAIModelConfig::default()) else {
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
#[ignore = "live openai"]
async fn live_stateful_responses() {
    let config = OpenAIModelConfig {
        stateful: Some(true),
        ..Default::default()
    };
    let Some(model) = model(OpenAIApi::Responses, config) else {
        return;
    };
    let mut agent = Agent::builder().model(model).printer(false).build().unwrap();
    let first = agent
        .invoke("My name is Alice. Reply with one word.")
        .await
        .unwrap();
    assert_eq!(first.stop_reason(), StopReason::EndTurn);
    assert!(
        agent.messages().is_empty(),
        "a stateful model keeps no local history"
    );
    let result = agent
        .invoke("What is my name? Reply with one word.")
        .await
        .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert!(
        agent.messages().is_empty(),
        "a stateful model keeps no local history"
    );
    assert!(
        serde_json::to_string(result.last_message())
            .unwrap()
            .to_lowercase()
            .contains("alice")
    );
}

#[tokio::test]
#[ignore = "live openai"]
async fn live_cache_hit() {
    let config = OpenAIModelConfig {
        cache_config: Some(CacheConfig::default()),
        ..Default::default()
    };
    let Some(model) = model(OpenAIApi::Responses, config) else {
        return;
    };
    let context = format!("Context information: {}", "repeat ".repeat(3000));
    let request = || ModelRequest {
        messages: vec![user(&context)],
        system_prompt: Some("You are a helpful assistant. Reply with one word.".into()),
        agent_metadata: Some(strands::agent::agent_metadata::AgentMetadata {
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
