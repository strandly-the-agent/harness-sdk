//! `#[tool]` and `#[tools]` expansions at run time, the Rust form of the TS Zod tool cases
//! (strands-ts/src/tools/__tests__/zod-tool.test.ts).

use std::sync::Arc;

use serde_json::json;

use crate::error::ToolValidationError;
use crate::tools::fixtures::mock_context;
use crate::tools::{Tool, ToolContext, ToolError, ToolOutcome, ToolStreamEvent};
use crate::types::json::JsonValue;
use crate::types::messages::ToolResultBlock;
use crate::types::tools::{ToolResultStatus, ToolUse};
use crate::{tool, tools};

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name.
    city: String,
    unit: Option<String>,
) -> Result<String, ToolError> {
    tokio::task::yield_now().await;
    Ok(format!(
        "Sunny in {city}{}",
        unit.map(|unit| format!(" ({unit})")).unwrap_or_default()
    ))
}

/// Add two numbers on the blocking pool.
#[tool(name = "add-numbers")]
fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Ask for confirmation.
#[tool]
fn confirm(#[tool(context)] ctx: &ToolContext) -> Result<JsonValue, ToolError> {
    Ok(ctx.interrupt("confirm")?)
}

/// Read the agent id from an owned context.
#[tool]
async fn agent_id(#[tool(context)] ctx: ToolContext) -> String {
    ctx.agent().id().to_owned()
}

/// Fail with an SDK error.
#[tool]
async fn fail() -> crate::Result<()> {
    Err(ToolValidationError::new("bad input").into())
}

/// Panic on the blocking pool.
#[tool]
fn explode() -> u8 {
    panic!("tool panic")
}

struct Counter {
    base: u32,
}

#[tools]
impl Counter {
    /// Add to the base.
    #[tool]
    async fn add_to(&self, amount: u32) -> u32 {
        tokio::task::yield_now().await;
        self.base + amount
    }

    /// Read the base and the tool use id.
    #[tool(name = "read-base")]
    fn read(&self, #[tool(context)] ctx: &ToolContext) -> String {
        format!("{}:{}", self.base, ctx.tool_use().tool_use_id)
    }
}

async fn run(tool: &dyn Tool, input: JsonValue) -> (Vec<ToolStreamEvent>, ToolOutcome) {
    let tool_use = ToolUse::new(tool.name(), "t-1", input);
    tool.stream(mock_context(tool_use.clone()))
        .collect_outcome(&tool_use)
        .await
}

async fn result(tool: &dyn Tool, input: JsonValue) -> ToolResultBlock {
    match run(tool, input).await {
        (events, ToolOutcome::Result(result)) if events.is_empty() => result,
        other => panic!("expected a result, got {other:?}"),
    }
}

fn content(result: &ToolResultBlock) -> JsonValue {
    serde_json::to_value(&result.content).unwrap()
}

#[tokio::test]
async fn an_async_tool_runs_with_validated_input() {
    let ok = result(&GetWeather, json!({ "city": "Lisbon", "unit": "C" })).await;
    assert_eq!(ok.status, ToolResultStatus::Success);
    assert_eq!(content(&ok), json!([{ "text": "Sunny in Lisbon (C)" }]));
    assert_eq!(
        content(&result(&GetWeather, json!({ "city": "Oslo" })).await),
        json!([{ "text": "Sunny in Oslo" }])
    );
    let _ = get_weather;
}

#[tokio::test]
async fn invalid_input_is_a_validation_error_result() {
    let invalid = result(&GetWeather, json!({ "city": 3 })).await;
    assert_eq!(invalid.status, ToolResultStatus::Error);
    assert_eq!(
        content(&invalid),
        json!([{ "text": "Error: /city: 3 is not of type \"string\"" }])
    );
    assert!(invalid.error().unwrap().is::<ToolValidationError>());
    let missing = result(&GetWeather, json!({})).await;
    assert_eq!(
        content(&missing),
        json!([{ "text": "Error: \"city\" is a required property" }])
    );
}

#[tokio::test(flavor = "current_thread")]
async fn a_sync_tool_runs_on_the_blocking_pool() {
    assert_eq!(Add.name(), "add-numbers");
    assert_eq!(
        content(&result(&Add, json!({ "a": 2, "b": 40 })).await),
        json!([{ "text": "42" }])
    );
    assert_eq!(add(1, 1), 2);
}

#[tokio::test]
async fn an_interrupt_pauses_the_call() {
    let (events, outcome) = run(&Confirm, json!({})).await;
    assert!(events.is_empty());
    let ToolOutcome::Interrupt(interrupt) = outcome else {
        panic!("expected an interrupt")
    };
    assert_eq!(interrupt.id, "tool:t-1:confirm");
    let _ = confirm;
}

#[tokio::test]
async fn the_context_can_be_taken_by_value() {
    assert_eq!(
        content(&result(&AgentId, json!({})).await),
        json!([{ "text": "mock-agent" }])
    );
    let _ = agent_id;
}

#[tokio::test]
async fn an_sdk_error_becomes_an_error_result() {
    let failed = result(&Fail, json!({})).await;
    assert_eq!(content(&failed), json!([{ "text": "Error: bad input" }]));
    let _ = fail;
}

#[tokio::test]
#[should_panic(expected = "tool panic")]
async fn a_panicking_sync_tool_resumes_the_panic_for_the_executor() {
    let _ = explode;
    let _ = run(&Explode, json!({})).await;
}

#[tokio::test]
async fn method_tools_share_the_instance() {
    let counter = Arc::new(Counter { base: 10 });
    let add_to = counter.add_to_tool();
    assert_eq!(add_to.name(), "add_to");
    assert_eq!(add_to.description(), "Add to the base.");
    assert_eq!(
        content(&result(add_to.as_ref(), json!({ "amount": 5 })).await),
        json!([{ "text": "15" }])
    );
    let read = counter.read_tool();
    assert_eq!(read.name(), "read-base");
    assert_eq!(
        content(&result(read.as_ref(), json!({})).await),
        json!([{ "text": "10:t-1" }])
    );
    assert_eq!(counter.tools().len(), 2);
    assert_eq!(Arc::strong_count(&counter), 3);
}

#[test]
fn the_tool_spec_has_the_normalized_schema() {
    assert_eq!(
        serde_json::to_value(GetWeather.tool_spec()).unwrap(),
        json!({
            "name": "get_weather",
            "description": "Get the current weather for a city.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "city": { "type": "string", "description": "City name." },
                    "unit": { "type": "string" },
                },
                "required": ["city"],
                "additionalProperties": false,
            },
        })
    );
}
