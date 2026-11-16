use strands::tool;

/// Takes the context mutably.
#[tool]
fn mutate(#[tool(context)] ctx: &mut strands::ToolContext) -> String {
    let _ = ctx;
    String::new()
}

fn main() {}
