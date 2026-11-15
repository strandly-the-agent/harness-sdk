use strands::tool;

struct Counter;

impl Counter {
    /// Reads the counter.
    #[tool]
    fn read(&self) -> u32 {
        0
    }
}

fn main() {}
