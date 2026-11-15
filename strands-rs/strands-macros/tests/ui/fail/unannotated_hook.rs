use strands::hooks::{HookRegistry, MessageAddedEvent};

fn main() {
    let registry = HookRegistry::new();
    registry.add_hook(|event| {
        let _: &mut MessageAddedEvent<'_> = event;
    });
}
