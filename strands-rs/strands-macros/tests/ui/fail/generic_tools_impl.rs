use strands::tools;

struct Store<T>(T);

#[tools]
impl<T: Send + Sync + 'static> Store<T> {
    /// Count the items.
    #[tool]
    fn count(&self) -> u32 {
        0
    }
}

fn main() {}
