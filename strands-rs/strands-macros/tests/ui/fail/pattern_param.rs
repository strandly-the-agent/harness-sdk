use strands::tool;

/// Adds a pair.
#[tool]
fn add((left, right): (i32, i32)) -> i32 {
    left + right
}

fn main() {}
