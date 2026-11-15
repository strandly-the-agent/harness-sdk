use strands::{ToolError, tool};

struct NotSerialize;

/// Returns something that can't be serialized.
#[tool]
fn opaque(value: String) -> Result<NotSerialize, ToolError> {
    let _ = value;
    Ok(NotSerialize)
}

fn main() {}
