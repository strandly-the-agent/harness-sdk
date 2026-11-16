use strands::tools;

struct Store;

#[tools(list = "count_tool")]
impl Store {
    /// Count the items.
    #[tool]
    fn count(&self) -> u32 {
        0
    }
}

fn main() {}
