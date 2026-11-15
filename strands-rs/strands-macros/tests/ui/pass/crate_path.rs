mod sdk {
    pub use ::strands::*;
}

/// Echo the text.
#[sdk::tool(crate = "crate::sdk")]
async fn echo(
    /// The text.
    text: String,
) -> Result<String, sdk::ToolError> {
    Ok(text)
}

struct Store;

#[sdk::tools(crate = "crate::sdk")]
impl Store {
    /// Count the items.
    #[tool]
    fn count(&self) -> u32 {
        0
    }
}

fn main() {
    use sdk::Tool;
    assert_eq!(Echo.name(), "echo");
    assert_eq!(Echo.tool_spec().input_schema().cloned().unwrap_or_default()["required"][0], "text");
    let store = std::sync::Arc::new(Store);
    assert_eq!(store.tools().len(), 1);
    assert_eq!(store.count_tool().name(), "count");
}
