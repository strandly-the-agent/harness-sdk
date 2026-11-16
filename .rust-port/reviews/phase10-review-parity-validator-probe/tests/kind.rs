//! The fixture's `kind` and `status` members, which `telemetry/tests.rs::fixture` removes before comparing.
use std::sync::Arc;

use opentelemetry::trace::{SpanKind, Status};
use pv_probe::recording;
use serde_json::{Value, json};
use strands::Agent;
use strands::testing::MockMessageModel;
use strands::tools::FunctionTool;
use strands::types::messages::{ContentBlock, TextBlock, ToolUseBlock};

#[tokio::test(flavor = "current_thread")]
async fn kind_and_status_match_spans_stable() {
    let (recorder, _guard) = recording();
    let model = MockMessageModel::new();
    model.add_turn(vec![
        ToolUseBlock::new("get_weather", "tooluse_tel_1", json!({"city": "Lisbon"})).into(),
        ToolUseBlock::new("flaky_tool", "tooluse_tel_2", json!({})).into(),
    ]);
    model.add_turn(vec![ContentBlock::from(TextBlock::new("Lisbon is sunny."))]);
    let weather = FunctionTool::new("get_weather", "weather", |_input: Value, _ctx| async move {
        Ok::<_, strands::ToolError>("sunny".to_owned())
    });
    let flaky = FunctionTool::new("flaky_tool", "fails", |_input: Value, _ctx| async move {
        Err::<String, _>(strands::ToolError::msg("flaky_tool exploded"))
    });
    let mut agent = Agent::builder()
        .model(Arc::new(model) as Arc<dyn strands::models::model::Model>)
        .tools(vec![Arc::new(weather) as Arc<dyn strands::tools::Tool>, Arc::new(flaky)])
        .name("telemetry_agent")
        .build()
        .unwrap();
    agent.invoke("Weather in Lisbon?").await.unwrap();
    let spans = recorder.spans();
    let fixture: Vec<Value> = serde_json::from_str(
        &std::fs::read_to_string("/mnt/workspace/review-r10/strands-rs/conformance/fixtures/telemetry/spans_stable.json").unwrap(),
    )
    .unwrap();
    let actual: Vec<Value> = spans
        .iter()
        .map(|s| {
            let kind = match s.span_kind {
                SpanKind::Internal => 0,
                SpanKind::Server => 1,
                SpanKind::Client => 2,
                SpanKind::Producer => 3,
                SpanKind::Consumer => 4,
            };
            let status = match &s.status {
                Status::Unset => json!({"code": 0}),
                Status::Ok => json!({"code": 1}),
                Status::Error { description } => json!({"code": 2, "message": description.as_ref()}),
            };
            json!({"name": s.name, "kind": kind, "status": status})
        })
        .collect();
    let expected: Vec<Value> = fixture
        .iter()
        .map(|s| json!({"name": s["name"], "kind": s["kind"], "status": s["status"]}))
        .collect();
    println!("actual   {}", json!(actual));
    println!("expected {}", json!(expected));
    let sorted = |mut v: Vec<Value>| {
        v.sort_by_key(|s| s.to_string());
        v
    };
    assert_eq!(sorted(actual), sorted(expected));
}
