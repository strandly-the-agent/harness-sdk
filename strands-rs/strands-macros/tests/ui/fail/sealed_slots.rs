fn main() {
    let _ = strands::hooks::registry::Slots::default();
    let _ = std::mem::size_of::<strands::hooks::registry::Slot<strands::hooks::BeforeToolCallEvent>>();
}
