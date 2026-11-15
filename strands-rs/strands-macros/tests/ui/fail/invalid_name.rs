use strands::tool;

/// Has a space in its name.
#[tool(name = "bad name")]
fn spaced(value: String) -> String {
    value
}

fn main() {}
