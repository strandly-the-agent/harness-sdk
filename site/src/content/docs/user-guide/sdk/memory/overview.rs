//! Rust snippets for `memory/overview.mdx`.

async fn getting_started() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:getting_started]
    use std::sync::Arc;

    use strands_agents::memory::{MemoryManager, TestMemoryStore};
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Persists to ~/.strands/memory/notes.json by default.
    let store = TestMemoryStore::new("notes")?;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .memory_manager(MemoryManager::new(vec![Arc::new(store)])?)
        .build()?;
    // --8<-- [end:getting_started]

    let _ = agent;
    Ok(())
}
