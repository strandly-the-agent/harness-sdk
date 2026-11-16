//! A downstream crate that depends only on `strands` and `tokio`, compiled under a strict lint
//! profile, so generated code stays clean for users.
#![deny(
    warnings,
    missing_docs,
    clippy::pedantic,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::allow_attributes_without_reason
)]

use std::sync::Arc;

use strands::agent::AgentAsToolOptions;
use strands::hooks::{AfterToolCallEvent, BeforeToolCallEvent, HookRegistry};
use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::{Agent, JsonValue, StopReason, Tool, ToolContext, ToolError, tool, tool_list, tools};

/// Get the current weather for a city.
///
/// # Errors
///
/// Never; the weather is always sunny.
#[tool]
pub async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
) -> Result<String, ToolError> {
    Ok(format!("Sunny in {city}"))
}

/// Count the characters of a text.
#[tool(name = "count_chars")]
#[must_use]
pub fn count(
    #[tool(context)] ctx: &ToolContext,
    /// The text to count.
    text: String,
) -> usize {
    let _ = ctx.tool_use();
    text.chars().count()
}

/// Report the server time.
#[tool]
pub async fn server_time() -> u64 {
    0
}

/// A product store.
#[derive(Debug)]
pub struct Inventory {
    stock: u32,
}

#[tools]
impl Inventory {
    /// Look up stock for a product.
    ///
    /// # Errors
    ///
    /// Never.
    #[tool]
    pub async fn check_stock(
        &self,
        #[tool(context)] ctx: &ToolContext,
        /// Product SKU.
        sku: String,
    ) -> Result<u32, ToolError> {
        let _ = (ctx.tool_use(), sku);
        tokio::task::yield_now().await;
        Ok(self.stock)
    }

    /// Reserve units of a product.
    ///
    /// # Errors
    ///
    /// Fails when the stock is too low.
    #[tool]
    pub fn reserve(
        &self,
        /// Units to reserve.
        units: u32,
    ) -> Result<u32, ToolError> {
        self.stock
            .checked_sub(units)
            .ok_or_else(|| ToolError::msg("not enough stock"))
    }
}

#[test]
fn generated_tools_have_specs_and_register() {
    let inventory = Arc::new(Inventory { stock: 3 });
    let tools = tool_list![
        GetWeather,
        Count,
        ServerTime,
        inventory.check_stock_tool(),
        inventory.tools()
    ];
    assert_eq!(tools.len(), 5);
    assert_eq!(GetWeather.name(), "get_weather");
    assert_eq!(Count.name(), "count_chars");
    assert_eq!(GetWeather.description(), "Get the current weather for a city.");
    assert_eq!(
        inventory.reserve_tool().description(),
        "Reserve units of a product."
    );
    let spec = GetWeather.tool_spec();
    let description = spec
        .input_schema()
        .and_then(|schema| schema.pointer("/properties/city/description"))
        .and_then(|value| value.as_str());
    assert_eq!(description, Some("City name, for example \"Seattle\"."));

    let hooks = HookRegistry::new();
    let cleanup = hooks.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name() == "shell" {
            event.set_cancel("shell is disabled");
        }
    });
    assert!(!hooks.is_empty());
    cleanup.remove();
}

#[tokio::test]
async fn an_agent_runs_generated_tools_with_a_mock_model() -> Result<(), Box<dyn std::error::Error>> {
    let input: JsonValue = r#"{"city": "Seattle"}"#.parse()?;
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![
        ToolUseBlock::new("get_weather", "tool-1", input.clone()).into(),
    ]);
    model.add_turn(vec![TextBlock::new("It is sunny in Seattle.").into()]);
    let inventory = Arc::new(Inventory { stock: 3 });
    let mut agent = Agent::builder()
        .model(Arc::clone(&model))
        .system_prompt("You are a concise assistant.")
        .tools(tool_list![GetWeather, Count, inventory.tools()])
        .printer(false)
        .build()?;
    agent.add_hook(|event: &mut BeforeToolCallEvent| {
        if event.tool_use().name() == "shell" {
            event.set_cancel("shell is disabled");
        }
    });
    agent.add_async_hook::<AfterToolCallEvent>(|event| {
        Box::pin(async move {
            let _ = event.result().status();
        })
    });

    let result = agent.invoke("What's the weather in Seattle?").await?;
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(result.to_string(), "It is sunny in Seattle.");
    assert_eq!(model.requests().len(), 2);

    let direct = agent.tool("get_weather").invoke(input).await?;
    assert_eq!(direct.tool_use_id().get(..8), Some("tooluse_"));
    assert_eq!(agent.messages().len(), 7);

    let helper = Agent::builder()
        .model(MockMessageModel::new())
        .name("helper")
        .printer(false)
        .build()?;
    let parent = Agent::builder()
        .model(MockMessageModel::new())
        .tools([helper
            .as_tool(AgentAsToolOptions::builder().delegate(true).build())
            .unwrap()])
        .printer(false)
        .build()?;
    assert!(parent.tool_registry().get("helper").is_some());
    agent.shutdown().await?;
    Ok(())
}
