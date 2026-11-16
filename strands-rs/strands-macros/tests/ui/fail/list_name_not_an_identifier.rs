use strands::tools;

struct Store;

#[tools(list = "my tools")]
impl Store {
    /// Count the items.
    #[tool]
    fn count(&self) -> u32 {
        0
    }
}

fn main() {}
