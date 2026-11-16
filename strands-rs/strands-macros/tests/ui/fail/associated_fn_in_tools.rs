use strands::tools;

struct Store;

#[tools]
impl Store {
    /// Count the items.
    #[tool]
    fn count() -> u32 {
        0
    }
}

fn main() {}
