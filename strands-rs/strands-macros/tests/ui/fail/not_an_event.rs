use strands::hooks::HookRegistry;

fn main() {
    let registry = HookRegistry::new();
    registry.add_async_hook::<String>(|_event| Box::pin(async {}));
}
