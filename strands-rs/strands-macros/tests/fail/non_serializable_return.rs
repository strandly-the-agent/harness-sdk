use strands::tool;

struct Opaque;

/// Returns something that can't be serialized.
#[tool]
fn make_opaque() -> Opaque {
    Opaque
}

fn main() {}
