//! Snippets for `memory.mdx`.

use async_trait::async_trait;
use strands_agents::memory::{MemoryEntry, MemoryStore, SearchOptions};
use strands_agents::Result;

/// Stand-in for a consumer's own store; only `search` is required.
struct PostgresMemoryStore;

impl PostgresMemoryStore {
    fn new(_dsn: &str) -> Self {
        Self
    }
}

#[async_trait]
impl MemoryStore for PostgresMemoryStore {
    fn name(&self) -> &str {
        "postgres"
    }

    async fn search(
        &self,
        _query: &str,
        _options: &SearchOptions,
    ) -> Result<Vec<MemoryEntry>> {
        Ok(Vec::new())
    }
}

async fn memory_dir() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:memory_dir]
    use strands_harness::{
        create_harness, HarnessAgentOptions, MemoryConfig, MemoryOption,
    };

    let agent = create_harness(HarnessAgentOptions {
        memory: MemoryOption::Config(MemoryConfig {
            dir: Some("/var/lib/agent/memory".into()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:memory_dir]

    let _ = agent;
    Ok(())
}

async fn memory_stores() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:memory_stores]
    use std::sync::Arc;

    use strands_harness::{
        create_harness, HarnessAgentOptions, MemoryConfig, MemoryOption,
    };

    // PostgresMemoryStore is your own MemoryStore implementation.
    let agent = create_harness(HarnessAgentOptions {
        memory: MemoryOption::Config(MemoryConfig {
            stores: vec![Arc::new(PostgresMemoryStore::new("postgres://..."))],
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:memory_stores]

    let _ = agent;
    Ok(())
}

async fn memory_off() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:memory_off]
    use strands_harness::{create_harness, HarnessAgentOptions, MemoryOption};

    let agent = create_harness(HarnessAgentOptions {
        memory: MemoryOption::Off,
        ..Default::default()
    })
    .await?;
    // --8<-- [end:memory_off]

    let _ = agent;
    Ok(())
}

async fn memory_flush_scope(task: &str) -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:memory_flush_scope]
    use strands_harness::{create_harness, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions::default()).await?;
    let result = agent.invoke(task).await;
    // The agent has no shutdown hook, so flush the manager before the scope exits.
    if let Some(memory) = agent.memory_manager() {
        memory.flush().await?;
    }
    result?;
    // --8<-- [end:memory_flush_scope]

    Ok(())
}

async fn memory_flush(
    agent: &strands_agents::Agent,
) -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:memory_flush]
    if let Some(memory) = agent.memory_manager() {
        memory.flush().await?;
    }
    // --8<-- [end:memory_flush]

    Ok(())
}
