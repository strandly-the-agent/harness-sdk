use strands::{ToolContext, tool};

/// Forgets to mark its context.
#[tool]
fn unmarked(ctx: &ToolContext) -> String {
    let _ = ctx;
    String::new()
}

fn main() {}
