//! Live Bedrock tests (TS checklist: strands-ts/test/integ/models/bedrock.test.ts). They call
//! Bedrock with the default AWS credential chain and skip with a message when no credentials
//! resolve. Run with `cargo test -p strands --all-features -- --ignored live_`.
#![cfg(feature = "bedrock")]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use aws_config::BehaviorVersion;
use aws_credential_types::provider::ProvideCredentials as _;
use futures::StreamExt as _;
use serde_json::json;
use strands::models::bedrock::{BedrockModel, BedrockModelConfig};
use strands::models::model::{CacheConfig, CacheStrategy, Model, ModelRequest};
use strands::models::streaming::ModelStreamEvent;
use strands::types::messages::{ContentBlock, Message, Role, StopReason, TextBlock};
use strands::types::tools::ToolSpec;
use strands::{Agent, InvokeOptions, tool};

/// A small model the TS integration tests use.
const MODEL_ID: &str = "us.anthropic.claude-haiku-4-5-20251001-v1:0";
const REGION: &str = "us-west-2";

#[cfg(test)]
async fn credentials_available() -> bool {
    let config = aws_config::defaults(BehaviorVersion::latest()).load().await;
    let Some(provider) = config.credentials_provider() else {
        eprintln!("skipping: no AWS credentials provider");
        return false;
    };
    if provider.provide_credentials().await.is_err() {
        eprintln!("skipping: AWS credentials did not resolve");
        return false;
    }
    true
}

#[cfg(test)]
fn model(config: BedrockModelConfig) -> BedrockModel {
    BedrockModel::new(strands::models::bedrock::BedrockModelOptions {
        config: BedrockModelConfig {
            model_id: Some(MODEL_ID.into()),
            max_tokens: Some(200),
            ..config
        },
        region: Some(REGION.into()),
        ..Default::default()
    })
}

#[cfg(test)]
fn user(text: &str) -> Message {
    Message::new(Role::User, vec![TextBlock::new(text).into()])
}

#[cfg(test)]
async fn collect_events(model: &BedrockModel, request: ModelRequest) -> Vec<ModelStreamEvent> {
    let mut stream = model.stream(request);
    let mut events = Vec::new();
    while let Some(event) = stream.next().await {
        events.push(event.expect("the model call succeeds"));
    }
    events
}

#[cfg(test)]
fn stop_reason(events: &[ModelStreamEvent]) -> Option<StopReason> {
    events.iter().find_map(|event| match event {
        ModelStreamEvent::MessageStop(stop) => Some(stop.stop_reason().clone()),
        _ => None,
    })
}

#[cfg(test)]
fn usage(events: &[ModelStreamEvent]) -> strands::models::streaming::Usage {
    events
        .iter()
        .find_map(|event| match event {
            ModelStreamEvent::Metadata(metadata) => metadata.usage().cloned(),
            _ => None,
        })
        .expect("a metadata event with usage")
}

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
) -> Result<String, strands::ToolError> {
    Ok(format!("Sunny and 21 degrees in {city}"))
}

#[derive(Debug, serde::Deserialize, schemars::JsonSchema)]
struct Person {
    name: String,
    age: u32,
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_text_with_the_converse_api() {
    if !credentials_available().await {
        return;
    }
    let model = model(BedrockModelConfig {
        stream: Some(false),
        ..Default::default()
    });
    let events = collect_events(
        &model,
        ModelRequest {
            messages: vec![user("Reply with one word.")],
            ..Default::default()
        },
    )
    .await;
    assert_eq!(stop_reason(&events), Some(StopReason::EndTurn));
    assert!(
        events
            .iter()
            .any(|event| event.text_delta().is_some_and(|text| !text.is_empty()))
    );
    let usage = usage(&events);
    assert!(usage.input_tokens() > 0 && usage.output_tokens() > 0);
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_streaming_events_and_max_tokens() {
    if !credentials_available().await {
        return;
    }
    let model = model(BedrockModelConfig::default());
    let events = collect_events(
        &model,
        ModelRequest {
            messages: vec![user("Count from 1 to 5, one number per line.")],
            ..Default::default()
        },
    )
    .await;
    assert!(matches!(events.first(), Some(ModelStreamEvent::MessageStart(_))));
    let deltas = events.iter().filter(|event| event.text_delta().is_some()).count();
    assert!(deltas >= 1);
    assert_eq!(stop_reason(&events), Some(StopReason::EndTurn));

    let short = model_with_max_tokens(5);
    let events = collect_events(
        &short,
        ModelRequest {
            messages: vec![user("Write a long story about a lighthouse.")],
            ..Default::default()
        },
    )
    .await;
    assert_eq!(stop_reason(&events), Some(StopReason::MaxTokens));
}

#[cfg(test)]
fn model_with_max_tokens(max_tokens: u64) -> BedrockModel {
    BedrockModel::new(strands::models::bedrock::BedrockModelOptions {
        config: BedrockModelConfig {
            model_id: Some(MODEL_ID.into()),
            max_tokens: Some(max_tokens),
            ..Default::default()
        },
        region: Some(REGION.into()),
        ..Default::default()
    })
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_tool_round_trip_through_an_agent() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(model(BedrockModelConfig::default()))
        .system_prompt("Use the available tools to answer.")
        .tools([GetWeather])
        .printer(false)
        .build()
        .unwrap();
    let result = agent.invoke("What is the weather in Seattle?").await.unwrap();
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
    assert_eq!(
        tool_use.input()["city"]
            .as_str()
            .map(str::to_lowercase)
            .as_deref(),
        Some("seattle")
    );
    let tool_result = blocks
        .iter()
        .find_map(|block| match block {
            ContentBlock::ToolResult(result) => Some(result),
            _ => None,
        })
        .expect("the tool ran");
    assert_eq!(tool_result.status().as_str(), "success");
    let result_json = serde_json::to_string(tool_result.content()).unwrap();
    assert!(result_json.contains("Sunny and 21 degrees in"));
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_structured_output() {
    if !credentials_available().await {
        return;
    }
    let mut agent = Agent::builder()
        .model(model(BedrockModelConfig::default()))
        .printer(false)
        .build()
        .unwrap();
    let person: Person = agent
        .invoke_with(
            "Generate a person named Ada who is 36 years old.",
            InvokeOptions::default().structured_output_schema::<Person>(),
        )
        .await
        .unwrap()
        .structured_output()
        .unwrap();
    assert_eq!(person.name, "Ada");
    assert_eq!(person.age, 36);
    agent.shutdown().await.unwrap();
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_cache_hit_with_tools_and_messages() {
    if !credentials_available().await {
        return;
    }
    let model = model(BedrockModelConfig {
        cache_config: Some(CacheConfig {
            strategy: Some(CacheStrategy::Auto),
            ..Default::default()
        }),
        ..Default::default()
    });
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let context = format!("Context information: {} [test-{nonce}]", "hello ".repeat(5000));
    let tool = ToolSpec::new("lookup", "Look up a fact by key.").with_input_schema(json!({
        "type": "object",
        "properties": { "key": { "type": "string", "description": "The fact key." } },
        "required": ["key"],
    }));
    let request = || ModelRequest {
        messages: vec![user(&format!("{context}\n\nReply with the word ok."))],
        tool_specs: vec![tool.clone()],
        ..Default::default()
    };
    let first = usage(&collect_events(&model, request()).await);
    assert!(
        first.cache_write_input_tokens().unwrap_or(0) > 0,
        "first call writes the cache: {first:?}"
    );
    let second = usage(&collect_events(&model, request()).await);
    assert!(
        second.cache_read_input_tokens().unwrap_or(0) > 0,
        "second call reads the cache: {second:?}"
    );
}

#[tokio::test]
#[ignore = "live bedrock"]
async fn live_invalid_model_id_fails_with_a_model_error() {
    if !credentials_available().await {
        return;
    }
    let model = BedrockModel::new(strands::models::bedrock::BedrockModelOptions {
        config: BedrockModelConfig {
            model_id: Some("invalid-model-id-that-does-not-exist".into()),
            ..Default::default()
        },
        region: Some(REGION.into()),
        ..Default::default()
    });
    let mut stream = model.stream(ModelRequest {
        messages: vec![user("Hello")],
        ..Default::default()
    });
    let mut failure = None;
    while let Some(event) = stream.next().await {
        if let Err(error) = event {
            failure = Some(error);
            break;
        }
    }
    let failure = failure.expect("the call fails");
    assert!(failure.is_model_error(), "{}", failure.name());
}
