use strands::{ToolContext, tool};

/// Takes the context twice.
#[tool]
fn twice(#[tool(context)] first: &ToolContext, #[tool(context)] second: &ToolContext) -> String {
    let _ = (first, second);
    String::new()
}

fn main() {}
