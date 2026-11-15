//! Name overrides, plain returns, sync bodies, visibility and doc copying.

#![deny(missing_docs)]

use strands::{Tool, tool};

/// Adds two numbers.
///
/// Second paragraph.
#[tool(name = "add-numbers")]
pub fn add(a: i64, b: i64) -> i64 {
    a + b
}

/// Does nothing.
#[tool]
pub(crate) async fn r#noop() {}

fn main() {
    assert_eq!(Add.name(), "add-numbers");
    assert_eq!(Add.description(), "Adds two numbers.\n\nSecond paragraph.");
    assert_eq!(Noop.name(), "noop");
    assert_eq!(add(1, 2), 3);
    let _ = noop;
}
