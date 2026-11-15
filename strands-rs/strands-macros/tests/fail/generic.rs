use strands::tool;

/// Echoes any value.
#[tool]
fn echo<T: Clone>(value: T) -> T {
    value
}

fn main() {}
