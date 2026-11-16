use strands::tools;

struct Store;

trait Count {
    fn count(&self) -> u32;
}

#[tools]
impl Count for Store {
    /// Count the items.
    #[tool]
    fn count(&self) -> u32 {
        0
    }
}

fn main() {}
