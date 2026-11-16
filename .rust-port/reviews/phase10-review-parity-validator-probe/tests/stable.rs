//! Run with no OTEL_* env (stable conventions).
use std::sync::Arc;

use pv_probe::{attrs, dump, recording};
use serde_json::{Value, json};
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::tools::StructuredOutputSchema;
use strands::types::messages::{ContentBlock, TextBlock};

fn text(t: &str) -> ContentBlock {
    TextBlock::new(t).into()
}

/// TS agent.ts:1716-1730: `tools: this.tools` is read before the structured-output tool is registered.
#[tokio::test(flavor = "current_thread")]
async fn agent_span_tools_exclude_the_structured_output_tool() {
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![text("Plain text, no tool call")]);
    model.add_turn(vec![text("{\"value\": 42}")]);
    let mut agent = Agent::builder()
        .model(Arc::new(model) as Arc<dyn strands::models::model::Model>)
        .name("so_agent")
        .structured_output_schema(StructuredOutputSchema::of::<Value>())
        .build()
        .unwrap();
    let result = agent.invoke("Test").await.unwrap();
    assert_eq!(result.structured_output_value(), Some(&json!({"value": 42})));
    let spans = recorder.spans();
    dump(&spans);
    let agent_span = spans.iter().find(|s| s.name == "invoke_agent so_agent").expect("agent span");
    let tools = attrs(agent_span, "gen_ai.agent.tools");
    assert!(tools.is_empty(), "TS records no gen_ai.agent.tools here (no tools registered); Rust recorded {tools:?}");
}

/// TS tracer.ts:304 `{ ...attributes, ...this._traceAttributes }`: a trace attribute replaces the span's own key.
#[tokio::test(flavor = "current_thread")]
async fn a_trace_attribute_overrides_the_spans_own_attribute_of_the_same_key() {
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![text("hi")]);
    let mut trace_attributes = indexmap::IndexMap::new();
    trace_attributes.insert("gen_ai.agent.name".to_owned(), strands::telemetry::AttributeValue::from("override"));
    trace_attributes.insert("gen_ai.operation.name".to_owned(), strands::telemetry::AttributeValue::from("custom_op"));
    let mut agent = Agent::builder()
        .model(Arc::new(model) as Arc<dyn strands::models::model::Model>)
        .name("real_name")
        .trace_attributes(trace_attributes)
        .build()
        .unwrap();
    agent.invoke("Test").await.unwrap();
    let spans = recorder.spans();
    dump(&spans);
    let agent_span = spans.iter().find(|s| s.name.starts_with("invoke_agent")).expect("agent span");
    assert_eq!(attrs(agent_span, "gen_ai.agent.name"), ["override"]);
    let chat = spans.iter().find(|s| s.name == "chat").expect("chat span");
    assert_eq!(attrs(chat, "gen_ai.operation.name"), ["custom_op"]);
}
