#![cfg(feature = "macros")]
//! The `#[tool]` attribute macro: generated spec, invocation, and error handling.

use std::sync::Arc;

use serde_json::json;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::{Tool, ToolContext};
use strands_agents::types::{ContentBlock, Role, StopReason, ToolResultStatus, ToolUse, ToolUseBlock};
use strands_agents::{tool, Agent, InvocationState, Message};
use tokio_util::sync::CancellationToken;

/// Current weather for a city.
#[tool]
async fn weather(city: String, days: Option<u32>) -> String {
    match days {
        Some(days) => format!("{city}: sunny for {days} days"),
        None => format!("{city}: sunny"),
    }
}

/// Divides two numbers.
#[tool]
async fn divide(a: f64, b: f64) -> Result<f64, String> {
    if b == 0.0 {
        return Err("division by zero".to_owned());
    }
    Ok(a / b)
}

/// Rolls a die.
#[tool]
async fn roll() -> u32 {
    4
}

/// Reports the id of the tool use that invoked it.
#[tool]
async fn echo_id(ctx: ToolContext) -> String {
    ctx.tool_use.tool_use_id.clone()
}

async fn agent() -> Agent {
    Agent::builder()
        .model_arc(MockMessageModel::shared(vec![MockResponse::message(
            Message::assistant("done"),
        )]))
        .build()
        .unwrap()
}

async fn invoke(tool: &dyn Tool, input: serde_json::Value) -> strands_agents::types::ToolResultBlock {
    let ctx = ToolContext::new(
        ToolUse {
            name: tool.name().to_owned(),
            tool_use_id: "t1".to_owned(),
            input,
        },
        agent().await,
        InvocationState::default(),
        CancellationToken::new(),
    );
    tool.invoke(ctx).await.unwrap()
}

fn text_of(result: &strands_agents::types::ToolResultBlock) -> String {
    serde_json::to_string(&result.content).unwrap()
}

#[tokio::test]
async fn derives_name_and_description_from_the_function() {
    let tool = WeatherTool::new();
    assert_eq!(tool.name(), "weather");
    assert_eq!(tool.description(), "Current weather for a city.");
    assert_eq!(tool.spec().name, "weather");
}

#[tokio::test]
async fn derives_the_input_schema_from_the_parameters() {
    let tool = WeatherTool::new();
    let schema = tool.spec().input_schema.clone().unwrap();
    assert_eq!(schema["type"], "object");
    assert_eq!(schema["properties"]["city"]["type"], "string");
    assert!(schema["properties"].get("days").is_some());
    assert_eq!(schema["required"], json!(["city"]));
}

#[tokio::test]
async fn optional_parameters_are_not_required() {
    let schema = WeatherTool::new().spec().input_schema.clone().unwrap();
    let required = schema["required"].as_array().unwrap();
    assert!(!required.iter().any(|value| value == "days"));
}

#[tokio::test]
async fn wraps_the_return_value_in_a_success_result() {
    let result = invoke(&WeatherTool::new(), json!({ "city": "Seattle" })).await;
    assert_eq!(result.status, ToolResultStatus::Success);
    assert!(text_of(&result).contains("Seattle: sunny"));
}

#[tokio::test]
async fn invalid_input_produces_an_error_result() {
    let result = invoke(&WeatherTool::new(), json!({ "city": 42 })).await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(text_of(&result).contains("Error: "));
}

#[tokio::test]
async fn err_from_the_function_produces_an_error_result() {
    let result = invoke(&DivideTool::new(), json!({ "a": 1.0, "b": 0.0 })).await;
    assert_eq!(result.status, ToolResultStatus::Error);
    assert!(text_of(&result).contains("Error: division by zero"));
}

#[tokio::test]
async fn ok_from_the_function_produces_a_success_result() {
    let result = invoke(&DivideTool::new(), json!({ "a": 6.0, "b": 3.0 })).await;
    assert_eq!(result.status, ToolResultStatus::Success);
    assert!(text_of(&result).contains('2'));
}

#[tokio::test]
async fn supports_tools_without_parameters() {
    let tool = RollTool::new();
    let schema = tool.spec().input_schema.clone().unwrap();
    assert_eq!(schema["type"], "object");
    assert!(schema.get("required").is_none());
    let result = invoke(&tool, json!({})).await;
    assert_eq!(result.status, ToolResultStatus::Success);
    assert!(text_of(&result).contains('4'));
}

#[tokio::test]
async fn passes_the_tool_context_through_and_hides_it_from_the_schema() {
    let tool = EchoIdTool::new();
    let schema = tool.spec().input_schema.clone().unwrap();
    assert_eq!(schema.get("properties"), None);
    let result = invoke(&tool, json!({})).await;
    assert!(text_of(&result).contains("t1"));
}

#[tokio::test]
async fn a_generated_tool_runs_in_the_agent_loop() {
    let model = MockMessageModel::shared(vec![
        MockResponse::with_stop_reason(
            Message::new(
                Role::Assistant,
                vec![ContentBlock::ToolUse(ToolUseBlock::new(
                    "weather",
                    "t1",
                    json!({ "city": "Seattle" }),
                ))],
            ),
            StopReason::ToolUse,
        ),
        MockResponse::message(Message::assistant("It is sunny in Seattle.")),
    ]);
    let agent = Agent::builder()
        .model_arc(model)
        .tool_ref(Arc::new(WeatherTool::new()))
        .build()
        .unwrap();

    let result = agent.invoke("weather in Seattle?").await.unwrap();

    assert_eq!(result.to_string(), "It is sunny in Seattle.");
    let messages = agent.messages();
    let ContentBlock::ToolResult(tool_result) = &messages[2].content[0] else {
        panic!("expected a tool result block");
    };
    assert_eq!(tool_result.status, ToolResultStatus::Success);
}
