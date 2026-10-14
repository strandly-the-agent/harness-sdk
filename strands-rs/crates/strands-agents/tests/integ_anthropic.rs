//! Live Anthropic integration tests (mirrors `strands-ts/test/integ/models/anthropic.test.ts`).
//! They call the real API and are `#[ignore]`d; run with:
//!
//! ```text
//! ANTHROPIC_API_KEY=sk-... cargo test --all-features --test integ_anthropic -- --ignored
//! ```

#![cfg(feature = "anthropic")]

use futures::StreamExt;
use strands_agents::models::{
    AggregatedItem, AnthropicModel, AnthropicModelConfig, ContentBlockDelta, CountTokensOptions, Model,
    ModelStreamEvent, StreamOptions,
};
use strands_agents::types::ToolSpec;
use strands_agents::{Agent, ContentBlock, Message, Role, StopReason};

const MODEL_ID: &str = "claude-sonnet-4-5";

/// `None` when `ANTHROPIC_API_KEY` is unset, so live tests skip instead of failing.
fn model(max_tokens: u64) -> Option<AnthropicModel> {
    if std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .is_none()
    {
        eprintln!("skipping: ANTHROPIC_API_KEY is not set");
        return None;
    }
    AnthropicModel::new(AnthropicModelConfig {
        model_id: Some(MODEL_ID.to_owned()),
        max_tokens: Some(max_tokens),
        ..Default::default()
    })
    .ok()
}

#[tokio::test]
#[ignore = "live Anthropic (ANTHROPIC_API_KEY)"]
async fn streams_text_and_reports_usage() {
    let Some(model) = model(64) else { return };
    let messages = vec![Message::new(
        Role::User,
        vec![ContentBlock::text("Say \"hello\".")],
    )];
    let options = StreamOptions::default();

    let mut text = String::new();
    let mut stop_reason = None;
    let mut usage = None;
    let mut stream = model.stream(&messages, &options);
    while let Some(event) = stream.next().await {
        match event.expect("stream event") {
            ModelStreamEvent::ContentBlockDelta {
                delta: ContentBlockDelta::Text { text: chunk },
            } => text.push_str(&chunk),
            ModelStreamEvent::Metadata { usage: u, .. } => usage = u,
            ModelStreamEvent::MessageStop { stop_reason: r, .. } => stop_reason = Some(r),
            _ => {}
        }
    }
    assert!(text.to_lowercase().contains("hello"), "got {text:?}");
    assert_eq!(stop_reason, Some(StopReason::EndTurn));
    assert!(usage.expect("usage").input_tokens > 0);
}

#[tokio::test]
#[ignore = "live Anthropic (ANTHROPIC_API_KEY)"]
async fn requests_a_tool_call() {
    let Some(model) = model(256) else { return };
    let options = StreamOptions {
        tool_specs: vec![ToolSpec {
            name: "get_weather".to_owned(),
            description: "Get the weather for a city".to_owned(),
            input_schema: Some(serde_json::json!({
                "type": "object",
                "properties": {"city": {"type": "string"}},
                "required": ["city"],
            })),
            output_schema: None,
            annotations: None,
        }],
        ..Default::default()
    };
    let messages = vec![Message::new(
        Role::User,
        vec![ContentBlock::text(
            "What is the weather in Seattle? Use the tool.",
        )],
    )];
    let mut aggregated = model.stream_aggregated(&messages, &options);
    let mut result = None;
    while let Some(item) = aggregated.next().await {
        if let AggregatedItem::Result(r) = item.expect("aggregated item") {
            result = Some(r);
        }
    }
    let result = result.expect("aggregated result");
    assert_eq!(result.stop_reason, StopReason::ToolUse);
    assert!(result
        .message
        .content
        .iter()
        .any(|b| matches!(b, ContentBlock::ToolUse(t) if t.name == "get_weather")));
}

#[tokio::test]
#[ignore = "live Anthropic (ANTHROPIC_API_KEY)"]
async fn counts_tokens_natively() {
    if std::env::var("ANTHROPIC_API_KEY")
        .ok()
        .filter(|k| !k.is_empty())
        .is_none()
    {
        eprintln!("skipping: ANTHROPIC_API_KEY is not set");
        return;
    }
    let model = AnthropicModel::new(AnthropicModelConfig {
        model_id: Some(MODEL_ID.to_owned()),
        max_tokens: Some(64),
        use_native_token_count: true,
        ..Default::default()
    })
    .expect("model");
    let messages = vec![Message::new(Role::User, vec![ContentBlock::text("Hello there")])];
    let count = model
        .count_tokens(&messages, &CountTokensOptions::default())
        .await
        .expect("count");
    assert!(count > 0);
}

#[tokio::test]
#[ignore = "live Anthropic (ANTHROPIC_API_KEY)"]
async fn runs_an_agent_end_to_end() {
    let Some(model) = model(128) else { return };
    let agent = Agent::builder()
        .model(model)
        .system_prompt("Answer in one word.")
        .build()
        .await
        .expect("agent");
    let result = agent
        .invoke("What is the capital of France?")
        .await
        .expect("invoke");
    assert!(
        result.text().to_lowercase().contains("paris"),
        "got {:?}",
        result.text()
    );
}
