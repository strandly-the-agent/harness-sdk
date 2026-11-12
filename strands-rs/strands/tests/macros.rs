//! `#[tool]` and `#[tools]` (the macro side of strands-ts/src/tools/__tests__/zod-tool.test.ts).

use std::sync::Arc;

use futures::StreamExt;
use serde_json::{Value, json};
use strands::testing::MockModel;
use strands::tools::{Tool, ToolStreamItem};
use strands::types::{ContentBlock, ToolUseBlock};
use strands::{ToolContext, ToolError, ToolOutcome, tool, tools};

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
    /// Days to forecast.
    days: Option<u32>,
) -> Result<String, ToolError> {
    Ok(format!("Sunny in {city} for {} days", days.unwrap_or(1)))
}

/// Adds two numbers.
#[tool(name = "add-numbers")]
fn add(a: f64, b: f64) -> f64 {
    a + b
}

/// Echoes the tool use id.
#[tool]
async fn whoami(#[tool(context)] ctx: &ToolContext, prefix: String) -> String {
    format!("{prefix}{}", ctx.tool_use().tool_use_id)
}

/// Always fails.
#[tool]
async fn fails() -> Result<u32, ToolError> {
    Err(ToolError::msg("nope"))
}

/// Asks for approval.
#[tool]
async fn approve(#[tool(context)] ctx: &ToolContext) -> Result<String, ToolError> {
    let answer = ctx.interrupt("approve")?;
    Ok(format!("answer {answer}"))
}

/// Panics.
#[tool]
fn explodes() -> u32 {
    panic!("sync tool exploded")
}

struct Inventory {
    stock: u32,
}

#[tools]
impl Inventory {
    /// Look up stock for a product.
    #[tool]
    async fn check_stock(&self, #[tool(context)] ctx: &ToolContext, sku: String) -> Result<u32, ToolError> {
        let _ = ctx.agent().app_state().get::<String>("region")?;
        Ok(if sku == "none" { 0 } else { self.stock })
    }

    /// Restocks a product.
    #[tool]
    fn restock(&self, amount: u32) -> u32 {
        self.stock + amount
    }

    fn helper(&self) -> u32 {
        self.stock
    }
}

async fn run(tool: &dyn Tool, input: Value) -> ToolOutcome {
    let context = strands::testing::tool_context(tool.name(), "t1", input);
    let mut stream = tool.stream(context);
    while let Some(item) = stream.next().await {
        if let ToolStreamItem::Outcome(outcome) = item {
            return outcome;
        }
    }
    panic!("no outcome")
}

fn content(outcome: &ToolOutcome) -> Value {
    match outcome {
        ToolOutcome::Result(result) => serde_json::to_value(&result.content).unwrap(),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn the_struct_is_named_after_the_function_and_the_doc_comment_is_the_description() {
    assert_eq!(GetWeather.name(), "get_weather");
    assert_eq!(GetWeather.description(), "Get the current weather for a city.");
    assert_eq!(
        GetWeather.tool_spec().input_schema,
        Some(json!({
            "type": "object",
            "properties": {
                "city": { "type": "string", "description": "City name, for example \"Seattle\"." },
                "days": { "type": "integer", "minimum": 0, "description": "Days to forecast." },
            },
            "required": ["city"],
            "additionalProperties": false,
        }))
    );
}

#[test]
fn the_name_attribute_overrides_the_model_facing_name() {
    assert_eq!(Add.name(), "add-numbers");
    assert_eq!(add(1.0, 2.0), 3.0);
}

#[tokio::test]
async fn async_tools_run_with_validated_input() {
    let outcome = run(&GetWeather, json!({ "city": "Seattle" })).await;
    assert_eq!(
        content(&outcome),
        json!([{ "text": "Sunny in Seattle for 1 days" }])
    );
}

#[tokio::test]
async fn invalid_input_is_an_error_result() {
    let outcome = run(&GetWeather, json!({ "city": 3 })).await;
    let text = content(&outcome)[0]["text"].as_str().unwrap().to_owned();
    assert!(
        text.starts_with("Error: Tool input validation failed: /city"),
        "{text}"
    );
}

#[tokio::test]
async fn sync_tools_run_on_a_blocking_thread() {
    let outcome = run(&Add, json!({ "a": 1, "b": 2.5 })).await;
    assert_eq!(content(&outcome), json!([{ "text": "3.5" }]));
}

#[tokio::test]
async fn the_context_parameter_is_passed_explicitly() {
    let outcome = run(&Whoami, json!({ "prefix": "id=" })).await;
    assert_eq!(content(&outcome), json!([{ "text": "id=t1" }]));
    assert!(
        Whoami.tool_spec().input_schema.unwrap()["properties"]
            .get("ctx")
            .is_none()
    );
}

#[tokio::test]
async fn errors_become_error_results() {
    let outcome = run(&Fails, json!({})).await;
    assert_eq!(content(&outcome), json!([{ "text": "Error: nope" }]));
}

#[tokio::test]
async fn interrupts_pass_through_the_question_mark() {
    let outcome = run(&Approve, json!({})).await;
    let ToolOutcome::Interrupt(interrupt) = outcome else {
        panic!("expected an interrupt");
    };
    assert_eq!(interrupt.id, "tool:t1:approve");
}

#[tokio::test]
async fn a_panicking_sync_tool_becomes_an_error_result() {
    let outcome = run(&Explodes, json!({})).await;
    let ToolOutcome::Result(result) = outcome else {
        panic!("expected a result");
    };
    assert_eq!(result.status, strands::tools::ToolResultStatus::Error);
}

#[tokio::test]
async fn tools_on_impl_blocks_share_the_receiver() {
    let inventory = Arc::new(Inventory { stock: 7 });
    assert_eq!(inventory.helper(), 7);
    let tools = inventory.tools();
    assert_eq!(tools.len(), 2);
    let check = inventory.check_stock_tool();
    assert_eq!(check.name(), "check_stock");
    assert_eq!(check.description(), "Look up stock for a product.");
    assert_eq!(
        content(&run(&check, json!({ "sku": "abc" })).await),
        json!([{ "text": "7" }])
    );
    let restock = inventory.restock_tool();
    assert_eq!(
        content(&run(&restock, json!({ "amount": 3 })).await),
        json!([{ "text": "10" }])
    );
}

#[tokio::test]
async fn macro_tools_run_inside_an_agent() {
    let model = MockModel::new();
    model.add_turn(vec![ContentBlock::ToolUse(ToolUseBlock::new(
        "get_weather",
        "t1",
        json!({ "city": "Lisbon" }),
    ))]);
    model.add_turn(vec!["It is sunny.".into()]);
    let mut agent = strands::Agent::builder()
        .model(model)
        .printer(false)
        .tools([GetWeather])
        .tools(Arc::new(Inventory { stock: 1 }).tools())
        .build()
        .unwrap();
    let result = agent.invoke("Weather?").await.unwrap();
    assert_eq!(result.to_string(), "It is sunny.");
    assert_eq!(agent.tools().len(), 3);
}
