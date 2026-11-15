//! The section 13 `#[tools]` target: tool methods on a struct that holds a client.

#![expect(clippy::print_stdout, reason = "an example prints its output")]

use std::collections::HashMap;
use std::fmt;
use std::sync::Arc;

use strands::{Agent, ToolContext, tools};

/// A stand-in for a database pool.
struct Pool {
    stock: HashMap<String, u32>,
}

/// The error a stock lookup fails with.
#[derive(Debug)]
struct UnknownSku(String);

impl fmt::Display for UnknownSku {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown sku {}", self.0)
    }
}

impl std::error::Error for UnknownSku {}

impl Pool {
    #[expect(clippy::unused_async, reason = "stands in for an async database call")]
    async fn stock(&self, sku: &str) -> Result<u32, UnknownSku> {
        self.stock
            .get(sku)
            .copied()
            .ok_or_else(|| UnknownSku(sku.to_owned()))
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
        sku: String,
    ) -> Result<u32, strands::ToolError> {
        let _ = ctx.agent().app_state().get::<String>("region");
        Ok(self.db.stock(&sku).await?)
    }
}

fn main() -> Result<(), strands::Error> {
    let db = Pool {
        stock: HashMap::from([("sku-1".to_owned(), 3)]),
    };
    let inventory = Arc::new(Inventory { db });
    let agent = Agent::builder().tools(inventory.tools()).build()?;
    for tool in agent.tool_registry().list() {
        println!("tool: {}", tool.name());
    }
    Ok(())
}
