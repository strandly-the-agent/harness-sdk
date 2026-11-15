use strands::tools;

struct Counter;

#[tools]
impl Counter {
    /// Consumes the counter.
    #[tool]
    fn consume(self) -> u32 {
        0
    }
}

fn main() {}
