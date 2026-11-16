//! Run with OTEL_SEMCONV_STABILITY_OPT_IN=gen_ai_latest_experimental.
use std::sync::Arc;

use pv_probe::{dump, recording};
use serde_json::json;
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::types::messages::{ContentBlock, Message, SystemPrompt, TextBlock};

fn text(t: &str) -> ContentBlock {
    TextBlock::new(t).into()
}

fn event_values(span: &opentelemetry_sdk::trace::SpanData, key: &str) -> Vec<String> {
    span.events
        .iter()
        .flat_map(|e| e.attributes.iter())
        .filter(|a| a.key.as_str() == key)
        .map(|a| a.value.to_string())
        .collect()
}

/// TS tracer.ts:1337-1338 `default: return { type: block.type }`; an UnknownBlock's type is 'unknownBlock'
/// (types/messages.ts:1170).
#[tokio::test(flavor = "current_thread")]
async fn an_unknown_block_is_reported_as_unknown_block() {
    assert_eq!(std::env::var("OTEL_SEMCONV_STABILITY_OPT_IN").as_deref(), Ok("gen_ai_latest_experimental"));
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![text("hi")]);
    let earlier: Vec<Message> = serde_json::from_value(json!([
        {"role": "user", "content": [{"text": "first"}, {"futureThing": {"a": 1}}]},
        {"role": "assistant", "content": [{"text": "ok"}]},
    ]))
    .unwrap();
    assert!(matches!(earlier[0].content()[1], ContentBlock::Unknown(_)));
    let mut agent = Agent::builder()
        .model(Arc::new(model) as Arc<dyn strands::models::model::Model>)
        .name("unknown_agent")
        .messages(earlier)
        .build()
        .unwrap();
    agent.invoke("Test").await.unwrap();
    let spans = recorder.spans();
    dump(&spans);
    let chat = spans.iter().find(|s| s.name == "chat").expect("chat span");
    let input = event_values(chat, "gen_ai.input.messages").join("\n");
    assert!(input.contains(r#"{"type":"unknownBlock"}"#), "TS writes {{\"type\":\"unknownBlock\"}}; Rust wrote {input}");
}

/// TS tracer.ts:1312-1313 `{ type: 'guard_content', text: block.text, image: block.image }`: JSON.stringify
/// drops the undefined member.
#[tokio::test(flavor = "current_thread")]
async fn a_guard_content_system_block_has_no_null_member() {
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![text("hi")]);
    let system_prompt: SystemPrompt = serde_json::from_value(json!([
        {"text": "be nice"},
        {"guardContent": {"text": {"qualifiers": ["query"], "text": "guarded"}}},
    ]))
    .unwrap();
    let mut agent = Agent::builder()
        .model(Arc::new(model) as Arc<dyn strands::models::model::Model>)
        .name("guard_agent")
        .system_prompt(system_prompt)
        .build()
        .unwrap();
    agent.invoke("Test").await.unwrap();
    let spans = recorder.spans();
    dump(&spans);
    let chat = spans.iter().find(|s| s.name == "chat").expect("chat span");
    let instructions = event_values(chat, "gen_ai.system_instructions").join("\n");
    assert!(instructions.contains("guard_content"), "{instructions}");
    assert!(!instructions.contains("null"), "TS omits the absent member; Rust wrote {instructions}");
}

/// TS tracer.ts:1313 passes `block.image` (a plain `{format, source: {bytes: Uint8Array}}`, types/messages.ts:961-991)
/// through jsonReplacer, which writes `"<replaced>"` for a Uint8Array (telemetry/json.ts).
#[tokio::test(flavor = "current_thread")]
async fn a_guard_content_image_in_system_instructions_has_its_bytes_replaced() {
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![text("hi")]);
    let system_prompt: SystemPrompt = serde_json::from_value(json!([
        {"guardContent": {"image": {"format": "png", "source": {"bytes": "aGVsbG8gd29ybGQ="}}}},
    ]))
    .unwrap();
    let mut agent = Agent::builder()
        .model(Arc::new(model) as Arc<dyn strands::models::model::Model>)
        .name("guard_image_agent")
        .system_prompt(system_prompt)
        .build()
        .unwrap();
    agent.invoke("Test").await.unwrap();
    let spans = recorder.spans();
    let chat = spans.iter().find(|s| s.name == "chat").expect("chat span");
    let instructions = event_values(chat, "gen_ai.system_instructions").join("\n");
    println!("{instructions}");
    assert!(instructions.contains("<replaced>"), "TS writes \"<replaced>\" for the bytes; Rust wrote {instructions}");
}
