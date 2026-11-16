use strands::tool;

/// Takes the context.
#[tool]
fn bare(#[tool] ctx: &strands::ToolContext) -> String {
    let _ = ctx;
    String::new()
}

fn main() {}
