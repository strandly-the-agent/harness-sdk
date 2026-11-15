use strands::{Tool, ToolContext, ToolError, tool};

/// Look up a word.
#[tool]
async fn lookup(
    /// The word to look up.
    word: String,
    #[tool(context)] ctx: &ToolContext,
    /// How many senses to return.
    limit: Option<u32>,
) -> Result<String, ToolError> {
    let _ = (ctx.tool_use(), limit);
    Ok(word)
}

fn main() {
    let spec = Lookup.tool_spec();
    let schema = spec.input_schema().cloned().unwrap_or_default();
    assert_eq!(schema["properties"]["word"]["description"], "The word to look up.");
    assert_eq!(schema["properties"]["limit"]["description"], "How many senses to return.");
    assert_eq!(schema["required"], strands::JsonValue::Array(vec!["word".into()]));
    assert!(schema["properties"].get("ctx").is_none());
}
