//! Rust snippets for `context-management.mdx`.

use strands_agents::Result;

async fn basic() -> Result<()> {
    // --8<-- [start:basic]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, ContextManagerOption};

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .context_manager(ContextManagerOption::Auto)
        .build()?;
    // --8<-- [end:basic]
    let _ = agent;
    Ok(())
}

async fn storage_backends() -> Result<()> {
    // --8<-- [start:storage_backends]
    use std::sync::Arc;

    use strands_agents::context_manager::{StashConfig, StashOption};
    use strands_agents::storage::{LocalFileStorage, S3Storage};

    // Local filesystem
    let stash_local = StashOption::Config(StashConfig {
        storage: Some(Arc::new(LocalFileStorage::new("./artifacts/"))),
        retrieval_tool: true,
    });

    // S3, using ambient AWS credentials
    let s3 = S3Storage::builder("my-bucket").prefix("agent-stash/").build().await?;
    let stash_s3 = StashOption::Config(StashConfig {
        storage: Some(Arc::new(s3)),
        retrieval_tool: true,
    });
    // --8<-- [end:storage_backends]
    let _ = (stash_local, stash_s3);
    Ok(())
}
