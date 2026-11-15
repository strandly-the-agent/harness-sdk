use strands::tool;

/// Takes the context twice.
#[tool]
fn twice(#[tool(context)] a: &strands::ToolContext, #[tool(context)] b: &strands::ToolContext) -> bool {
    let _ = (a, b);
    true
}

fn main() {}
