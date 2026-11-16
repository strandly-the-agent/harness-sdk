use std::sync::Arc;
use strands::tools;

struct Store;

#[tools]
impl Store {
    /// Counts.
    #[tool]
    fn count(&self) -> u32 {
        0
    }
}

#[cfg(all())]
#[tools]
impl Store {
    /// Gated.
    #[tool]
    fn hidden(&self) -> u32 {
        1
    }
}

fn main() {
    let _ = Arc::new(Store).tools();
}
