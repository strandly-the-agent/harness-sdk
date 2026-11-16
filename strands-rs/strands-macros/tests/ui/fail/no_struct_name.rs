use strands::tool;

/// Adds one.
#[tool(name = "inc")]
fn _1(n: i32) -> i32 {
    n + 1
}

/// Does nothing.
#[tool(name = "noop")]
fn __() {}

fn main() {}
