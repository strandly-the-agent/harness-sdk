use strands::tools;

struct Store;

#[tools]
impl Store {
    /// Count the items.
    #[tool]
    fn count(&self) -> u32 {
        0
    }

    /// A tool of some builds.
    #[cfg_attr(all(), tool)]
    fn sometimes(&self) -> u32 {
        1
    }
}

fn main() {}
