use strands::hooks::HookRegistry;

struct NotAnEvent;

fn main() {
    let registry = HookRegistry::new();
    registry.add_async_hook::<NotAnEvent>(|_event| Box::pin(async {}));
}
