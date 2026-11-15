//! `#[tools]` generates one constructor per `#[tool]` method plus `tools()`.

#![deny(missing_docs)]

use std::sync::Arc;

use strands::{ToolContext, ToolError, tools};

/// A product inventory.
pub struct Inventory {
    stock: u32,
}

#[tools]
impl Inventory {
    /// Look up stock for a product.
    #[tool]
    pub async fn check_stock(
        &self,
        #[tool(context)] ctx: &ToolContext,
        /// The product id.
        sku: String,
    ) -> Result<u32, ToolError> {
        let _ = ctx.agent().app_state().get::<String>("region")?;
        Ok(if sku.is_empty() { 0 } else { self.stock })
    }

    /// Restock a product.
    #[strands::tool(name = "restock")]
    pub fn add_stock(&self, amount: u32) -> u32 {
        self.stock + amount
    }

    /// Not a tool.
    pub fn helper(&self) -> u32 {
        self.stock
    }
}

fn main() {
    let inventory = Arc::new(Inventory { stock: 3 });
    assert_eq!(inventory.check_stock_tool().name(), "check_stock");
    assert_eq!(inventory.add_stock_tool().name(), "restock");
    let schema = strands::types::json::to_string(&inventory.check_stock_tool().tool_spec().input_schema).unwrap();
    assert!(schema.contains(r#""sku":{"type":"string","description":"The product id."}"#), "{schema}");
    assert_eq!(inventory.tools().len(), 2);
    assert_eq!(inventory.helper(), 3);
}
