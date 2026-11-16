use strands::tools;

struct Store;

#[tools]
impl Store {
    /// Count the tools.
    #[tool]
    fn tools(&self) -> u8 {
        0
    }
}

fn main() {}
