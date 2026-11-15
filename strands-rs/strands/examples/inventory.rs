//! Tools on a struct with `#[tools]` and a tool context (PROMPT.md section 13), run against a
//! scripted mock model so it works offline.

#![expect(clippy::print_stdout, reason = "examples print their results")]

use std::sync::Arc;

use strands::testing::MockMessageModel;
use strands::types::messages::{TextBlock, ToolUseBlock};
use strands::{Agent, JsonValue, ToolContext, tools};

/// A stand-in for a database pool.
struct Pool;

impl Pool {
    async fn stock(&self, sku: &str) -> Result<u32, std::num::ParseIntError> {
        sku.trim_start_matches("sku-").parse()
    }
}

struct Inventory {
    db: Pool,
}

#[tools]
impl Inventory {
    /// Look up stock for a product.
    #[tool]
    async fn check_stock(
        &self,
        #[tool(context)] ctx: &ToolContext,
        /// Product SKU.
        sku: String,
    ) -> Result<u32, strands::ToolError> {
        let _ = ctx.agent().app_state().get::<String>("region");
        Ok(self.db.stock(&sku).await?)
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let input: JsonValue = r#"{"sku": "sku-42"}"#.parse()?;
    let model = MockMessageModel::new();
    model.add_turn(vec![
        ToolUseBlock::new("check_stock", "tool-1", input.clone()).into(),
    ]);
    model.add_turn(vec![TextBlock::new("There are 42 in stock.").into()]);

    let inventory = Arc::new(Inventory { db: Pool });
    let mut agent = Agent::builder().model(model).tools(inventory.tools()).build()?;
    agent.invoke("How many sku-42 are left?").await?;

    let direct = agent.tool("check_stock").invoke(input).await?;
    println!("direct call: {:?}", direct.status());
    agent.shutdown().await?;
    Ok(())
}
