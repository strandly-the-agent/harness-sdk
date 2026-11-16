use strands::tool;

/// Only with the feature.
#[tool]
#[cfg(any())]
fn gone() -> u8 {
    1
}

fn main() {}
