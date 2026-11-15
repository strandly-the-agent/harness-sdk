use strands::hooks::{BeforeToolCallEvent, EventKind};

struct Mine;

impl EventKind for Mine {
    type Kind = BeforeToolCallEvent;
}

fn main() {}
