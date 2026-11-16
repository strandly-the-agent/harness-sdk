//! The run of the conformance generator's `telemetry()` case, compared with the fixtures no Rust test reads:
//! telemetry/agent_metrics.json and telemetry/agent_traces.json.
use std::sync::Arc;

use serde_json::{Value, json};
use strands::Agent;
use strands::models::streaming::Usage;
use strands::testing::{MockMessageModel, MockTurnOptions};
use strands::tools::FunctionTool;
use strands::types::messages::{ContentBlock, TextBlock, ToolUseBlock};

fn fixture(name: &str) -> Value {
    let path = format!("/mnt/workspace/review-r10/strands-rs/conformance/fixtures/telemetry/{name}.json");
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

/// Replaces values that differ per run (ids, clocks, durations, tracking ids, model latency) by their JSON type.
fn shape(value: &Value, key: &str) -> Value {
    match value {
        Value::Object(map) => Value::Object(map.iter().map(|(k, v)| (k.clone(), shape(v, k))).collect()),
        Value::Array(items) => Value::Array(items.iter().map(|v| shape(v, key)).collect()),
        Value::Null => Value::Null,
        other if matches!(key, "id" | "parentId" | "startTime" | "endTime" | "duration" | "trackingId" | "totalDuration" | "totalTime" | "latencyMs" | "timeToFirstByteMs") => {
            json!(match other { Value::String(_) => "string", Value::Number(_) => "number", _ => "other" })
        }
        other => other.clone(),
    }
}

#[tokio::test(flavor = "current_thread")]
async fn agent_metrics_and_traces_have_the_fixture_shape() {
    let usage = |i: u64, o: u64| MockTurnOptions::builder().usage(Usage::new(i, o, i + o)).build();
    let model = MockMessageModel::new();
    model.add_turn_with(
        vec![
            ToolUseBlock::new("get_weather", "tooluse_tel_1", json!({"city": "Lisbon"})).into(),
            ToolUseBlock::new("flaky_tool", "tooluse_tel_2", json!({})).into(),
        ],
        usage(30, 14),
    );
    model.add_turn_with(vec![ContentBlock::from(TextBlock::new("Lisbon is sunny."))], usage(12, 5));
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
    let result = agent.invoke("Weather in Lisbon?").await.unwrap();

    let metrics = serde_json::to_value(result.metrics()).unwrap();
    println!("RUST metrics: {metrics}");
    let traces = serde_json::to_value(result.traces()).unwrap();
    println!("RUST traces: {traces}");
    let mut expected_metrics = shape(&fixture("agent_metrics"), "");
    // The mock model reports no latency.
    let mut actual_metrics = shape(&metrics, "");
    for m in [&mut expected_metrics, &mut actual_metrics] {
        m.as_object_mut().unwrap().remove("accumulatedMetrics");
    }
    assert_eq!(actual_metrics, expected_metrics, "agent_metrics");
    let strip_metrics = |mut v: Value| {
        for cycle in v.as_array_mut().unwrap() {
            for child in cycle["children"].as_array_mut().unwrap() {
                if let Some(meta) = child["message"].get_mut("metadata").and_then(Value::as_object_mut) {
                    meta.remove("metrics");
                }
            }
        }
        v
    };
    assert_eq!(strip_metrics(shape(&traces, "")), strip_metrics(shape(&fixture("agent_traces"), "")), "agent_traces");
}
