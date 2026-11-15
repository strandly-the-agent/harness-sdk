//! `#[tool(context)]` passes the context, by reference or by value, in any position.

use strands::{Tool, ToolContext, ToolError, tool};

/// Reads the tool use id.
#[tool]
async fn context_first(#[tool(context)] ctx: &ToolContext, label: String) -> Result<String, ToolError> {
    Ok(format!("{label}:{}", ctx.tool_use().tool_use_id))
}

/// Takes the context by value, after the input.
#[tool]
fn context_last(count: u32, #[tool(context)] ctx: ToolContext) -> String {
    format!("{count}:{}", ctx.agent().id())
}

/// Has no input at all.
#[tool]
fn no_input(#[tool(context)] _ctx: &ToolContext) -> bool {
    true
}

fn main() {
    let schema = |tool: &dyn Tool| strands::types::json::to_string(&tool.tool_spec().input_schema).unwrap();
    assert_eq!(
        schema(&ContextFirst),
        r#"{"type":"object","properties":{"label":{"type":"string"}},"required":["label"],"additionalProperties":false}"#
    );
    assert!(schema(&ContextLast).contains(r#""required":["count"]"#));
    assert_eq!(schema(&NoInput), r#"{"type":"object","properties":{},"additionalProperties":false}"#);
    let _ = (context_first, context_last, no_input);
}
