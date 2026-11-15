use strands::tool;

/// Borrows its input.
#[tool]
fn shout(text: &str) -> String {
    text.to_uppercase()
}

fn main() {}
