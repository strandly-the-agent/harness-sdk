//! Rust snippets for `storage.mdx`.

use strands_agents::Result;

async fn agent_level() -> Result<()> {
    // --8<-- [start:agent_level]
    use std::sync::Arc;

    use strands_agents::models::BedrockModel;
    use strands_agents::session::FileStorage;
    use strands_agents::storage::S3Storage;
    use strands_agents::{Agent, ContextManagerOption, SessionManager};

    let storage = S3Storage::builder("my-bucket")
        .prefix("agents/prod/")
        .build()
        .await?;

    // The context manager's stash resolves from the agent's storage; the session
    // manager takes a SnapshotStorage of its own.
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .storage(storage)
        .session_manager(SessionManager::new(
            "my-session",
            Arc::new(FileStorage::new("./.strands/")),
        )?)
        .context_manager(ContextManagerOption::Auto)
        .build()?;
    // --8<-- [end:agent_level]
    let _ = agent;
    Ok(())
}

async fn per_plugin() -> Result<()> {
    // --8<-- [start:per_plugin]
    use std::sync::Arc;

    use strands_agents::context_manager::{StashConfig, StashOption};
    use strands_agents::models::BedrockModel;
    use strands_agents::session::S3SessionStorage;
    use strands_agents::storage::InMemoryStorage;
    use strands_agents::{Agent, ContextManagerConfig, ContextManagerOption, SessionManager};

    let snapshots = S3SessionStorage::new("my-bucket").await?;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .session_manager(SessionManager::new("my-session", Arc::new(snapshots))?)
        .context_manager(ContextManagerOption::Config(ContextManagerConfig {
            stash: StashOption::Config(StashConfig {
                storage: Some(Arc::new(InMemoryStorage::new())),
                retrieval_tool: true,
            }),
            ..Default::default()
        }))
        .build()?;
    // --8<-- [end:per_plugin]
    let _ = agent;
    Ok(())
}

fn in_memory() {
    // --8<-- [start:in_memory]
    use strands_agents::storage::InMemoryStorage;

    let storage = InMemoryStorage::new();
    // --8<-- [end:in_memory]
    let _ = storage;
}

fn local_file() {
    // --8<-- [start:local_file]
    use strands_agents::storage::LocalFileStorage;

    let storage = LocalFileStorage::new("./my-data/");
    // --8<-- [end:local_file]
    let _ = storage;
}

async fn s3() -> Result<()> {
    // --8<-- [start:s3]
    use strands_agents::storage::S3Storage;

    let storage = S3Storage::builder("my-bucket")
        .prefix("agents/prod/")
        .build()
        .await?;
    // --8<-- [end:s3]
    let _ = storage;
    Ok(())
}
