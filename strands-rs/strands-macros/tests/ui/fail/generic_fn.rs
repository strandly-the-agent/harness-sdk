use strands::tool;

/// Echoes any value.
#[tool]
fn echo<T: Clone>(value: T) -> String {
    let _ = value;
    String::new()
}

fn main() {}
