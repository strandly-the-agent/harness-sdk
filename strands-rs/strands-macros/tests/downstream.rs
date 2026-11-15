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

use strands::hooks::{BeforeToolCallEvent, HookRegistry};
use strands::{Tool, ToolContext, ToolError, tool, tool_list, tools};

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
