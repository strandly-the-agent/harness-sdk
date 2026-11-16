use std::sync::Arc;

use strands::Tool;

struct Inventory;

#[strands::tools]
impl Inventory {
    /// Look up stock.
    #[tool]
    fn check_stock(&self) -> u32 {
        3
    }
}

#[strands::tools(list = "admin_tools")]
impl Inventory {
    /// Reset the stock.
    #[tool]
    fn reset(&self) -> u32 {
        0
    }
}

fn main() {
    let inventory = Arc::new(Inventory);
    assert_eq!(inventory.tools().len(), 1);
    assert_eq!(inventory.admin_tools().len(), 1);
    assert_eq!(inventory.reset_tool().name(), "reset");
}
