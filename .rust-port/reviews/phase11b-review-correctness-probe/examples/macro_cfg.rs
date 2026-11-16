use std::sync::Arc;
use strands::{tool, tools};

struct Store;

#[tools]
impl Store {
    /// Counts.
    #[tool]
    fn count(&self) -> u32 {
        0
    }

    /// Only with the feature.
    #[cfg(any())]
    #[tool]
    fn hidden(&self) -> u32 {
        1
    }
}

fn main() {
    let _ = Arc::new(Store).tools();
}
