use strands::{tool, ToolContext};

/// Bumps.
#[tool]
fn bump(mut n: i32, context: String) -> i32 {
    n += 1;
    let _ = context;
    n
}

/// Uses the context.
#[tool]
async fn ctx(#[tool(context)] c: &ToolContext, r#type: String) -> String {
    let _ = c;
    r#type
}

/// Extern.
#[tool]
pub(crate) const extern "C" fn ext() -> u8 { 1 }

fn main() {
    let _ = (Bump, Ctx, Ext);
}
