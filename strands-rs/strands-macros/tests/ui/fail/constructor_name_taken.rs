use strands::tools;

struct Store;

#[tools]
impl Store {
    /// Count the items.
    #[tool]
    fn count(&self) -> u32 {
        0
    }

    fn count_tool(&self) -> u32 {
        1
    }
}

fn main() {}
