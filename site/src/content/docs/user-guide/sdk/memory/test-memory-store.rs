//! Rust snippets for `memory/test-memory-store.mdx`.

use strands_agents::Result;

async fn basic() -> Result<()> {
    // --8<-- [start:basic]
    use std::sync::Arc;

    use strands_agents::memory::{MemoryManager, TestMemoryStore};
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Persists to ~/.strands/memory/notes.json by default. Survives restarts.
    let store = TestMemoryStore::new("notes")?;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .memory_manager(MemoryManager::new(vec![Arc::new(store)])?)
        .build()?;
    // --8<-- [end:basic]
    let _ = agent;
    Ok(())
}

fn persistence() -> Result<()> {
    // --8<-- [start:persistence]
    use strands_agents::memory::{TestMemoryStore, TestMemoryStoreConfig};

    // Ephemeral: nothing is written to disk, and a fresh instance forgets everything.
    let scratch = TestMemoryStore::ephemeral("notes")?;

    // Explicit file location instead of the default under ~/.strands/memory/.
    let project = TestMemoryStore::with_config(TestMemoryStoreConfig {
        path: Some("./notes.json".to_string()),
        ..TestMemoryStoreConfig::new("notes")
    })?;
    // --8<-- [end:persistence]
    let _ = (scratch, project);
    Ok(())
}

async fn search_and_add() -> Result<()> {
    // --8<-- [start:search_and_add]
    use serde_json::json;
    use strands_agents::memory::{Metadata, TestMemoryStore};

    let store = TestMemoryStore::new("notes")?;

    // add_record returns the id of the stored (or already-present, on dedup) record.
    let metadata = Metadata::from([("category".to_string(), json!("travel"))]);
    let id = store.add_record("User prefers aisle seats", Some(&metadata)).await?;

    let results =
        store.search_memories("which seats does the user prefer?", None).await?;
    for entry in &results {
        let score = entry.metadata.as_ref().and_then(|m| m.get("_relevanceScore"));
        println!("{} {score:?}", entry.content);
    }
    // --8<-- [end:search_and_add]
    let _ = id;
    Ok(())
}

async fn extraction() -> Result<()> {
    // --8<-- [start:extraction]
    use std::sync::Arc;

    use strands_agents::memory::{
        ExtractionConfig, ModelExtractor, TestMemoryStore, TestMemoryStoreConfig,
    };
    use strands_agents::models::BedrockModel;

    // distill facts from the conversation, every 5 turns
    let extractor = ModelExtractor::new(Arc::new(BedrockModel::default_model().await?));

    let store = TestMemoryStore::with_config(TestMemoryStoreConfig {
        extraction: Some(ExtractionConfig::default().extractor(Arc::new(extractor))),
        ..TestMemoryStoreConfig::new("notes")
    })?;
    // --8<-- [end:extraction]
    let _ = store;
    Ok(())
}
