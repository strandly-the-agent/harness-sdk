use strands::tool;

/// Shows a value.
#[tool]
fn show(value: impl std::fmt::Display) -> String {
    value.to_string()
}

fn main() {}
