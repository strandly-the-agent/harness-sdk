//! Three ways to define a tool: `#[tool]` on a function, `#[tools]` on an impl block (tools that
//! share state), and `FunctionTool` for a tool assembled at run time. The agent then calls them,
//! and the program calls one directly.
//!
//! The default model is Amazon Bedrock's; AWS credentials and the region come from the default
//! chain.
//!
//! ```sh
//! cargo run -p strands --example tools
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use std::collections::HashMap;
use std::sync::Arc;

use serde_json::json;
use strands::tools::FunctionTool;
use strands::{Agent, ToolContext, ToolError, tool, tool_list, tools};

/// Convert a price in US dollars to euros.
#[tool]
fn usd_to_eur(
    /// The amount in US dollars.
    usd: f64,
) -> f64 {
    (usd * 0.92 * 100.0).round() / 100.0
}

struct Inventory {
    stock: HashMap<String, u32>,
}

#[tools]
impl Inventory {
    /// Look up how many units of a product are in stock.
    #[tool]
    async fn check_stock(
        &self,
        #[tool(context)] ctx: &ToolContext,
        /// Product SKU, for example "sku-42".
        sku: String,
    ) -> Result<u32, ToolError> {
        // Tools read (and write) the agent's app state through the context.
        let warehouse: Option<String> = ctx.agent().app_state().get("warehouse")?;
        println!(
            "[check_stock] {sku} in {}",
            warehouse.as_deref().unwrap_or("any warehouse")
        );
        self.stock
            .get(&sku)
            .copied()
            .ok_or_else(|| ToolError::msg(format!("unknown SKU: {sku}")))
    }
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct PriceInput {
    /// Product SKU.
    sku: String,
}

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let inventory = Arc::new(Inventory {
        stock: HashMap::from([("sku-42".to_owned(), 7)]),
    });
    let price = FunctionTool::typed(
        "get_price",
        "Get the price of a product in US dollars.",
        |input: PriceInput, _ctx| async move {
            let usd = if input.sku == "sku-42" { 19.99 } else { 5.0 };
            Ok::<_, ToolError>(json!({ "sku": input.sku, "usd": usd }))
        },
    );

    let mut agent = Agent::builder()
        .system_prompt("You are a concise shop assistant. Use the tools; never guess.")
        .tools(tool_list![UsdToEur, price])
        .tools(inventory.tools())
        .build()?;
    agent.app_state().set("warehouse", "LIS-1")?;

    let result = agent
        .invoke("How many sku-42 are in stock, and what does one cost in euros?")
        .await?;
    println!("\n{:?}", result.stop_reason());

    // A direct call runs the tool without the model and records it in the conversation.
    let direct = agent
        .tool("check_stock")
        .invoke(json!({ "sku": "sku-42" }))
        .await?;
    println!("direct call: {:?} {:?}", direct.status(), direct.content());

    agent.shutdown().await
}
