use std::sync::Arc;

use strands::{Tool, tools};

struct Store;

#[tools]
impl Store {
    /// Count the items.
    #[tool]
    fn count(&self) -> u32 {
        0
    }

    /// A tool of a build that never happens.
    #[cfg(any())]
    #[tool]
    fn hidden(&self) -> u32 {
        1
    }

    /// A tool of every build.
    #[cfg(all())]
    #[tool]
    fn shown(&self) -> u32 {
        2
    }
}

fn main() {
    let store = Arc::new(Store);
    let names: Vec<String> = [store.count_tool().name(), store.shown_tool().name()]
        .iter()
        .map(ToString::to_string)
        .collect();
    assert_eq!(names, ["count", "shown"]);
    assert_eq!(store.tools().len(), 2);
}
