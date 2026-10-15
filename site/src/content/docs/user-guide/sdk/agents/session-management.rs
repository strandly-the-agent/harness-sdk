//! Rust snippets for `session-management.mdx`. Each snippet has its own scope.

async fn basic_file_storage_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:basic_file_storage]
    use std::sync::Arc;

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::session::{FileStorage, SessionManager};
    use strands_agents::Agent;

    let session = SessionManager::new("test-session", Arc::new(FileStorage::new("./sessions/")))?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).session_manager(session).build()?;

    // Use the agent - all messages and state are automatically persisted
    agent.invoke("Hello!").await?; // This conversation is persisted
    // --8<-- [end:basic_file_storage]
    Ok(())
}

async fn session_as_plugin_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:session_as_plugin]
    use std::sync::Arc;

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::session::{FileStorage, SessionManager};
    use strands_agents::Agent;

    let session = SessionManager::new("test-session", Arc::new(FileStorage::new("./sessions/")))?;

    // Equivalent to passing via the session_manager field
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).plugin(Arc::new(session)).build()?;
    agent.invoke("Hello!").await?;
    // --8<-- [end:session_as_plugin]
    Ok(())
}

async fn session_id_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:session_id]
    use std::sync::Arc;

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::session::{FileStorage, SessionManager};
    use strands_agents::Agent;

    let session = SessionManager::new("user-123", Arc::new(FileStorage::new("./sessions/")))?;
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model.clone())
        .session_manager(session.clone())
        .build()?;

    println!("{}", agent.session_id()); // "user-123"
    println!("{}", session.session_id()); // "user-123"

    // Without a session manager, the id is a generated, non-persisted handle
    let plain = Agent::builder().model(model).build()?;
    println!("{}", plain.session_id()); // e.g. "a1b2c3d4"
    // --8<-- [end:session_id]
    Ok(())
}

async fn snapshot_trigger_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:snapshot_trigger]
    use std::sync::Arc;

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::session::{FileStorage, SessionManager, SessionManagerOptions};
    use strands_agents::Agent;

    let options = SessionManagerOptions {
        // Create an immutable snapshot after every 4 messages
        snapshot_trigger: Some(Arc::new(|agent: &Agent| agent.messages().len() % 4 == 0)),
        ..Default::default()
    };
    let session = SessionManager::with_options(
        "my-session",
        Arc::new(FileStorage::new("./sessions/")),
        options,
    )?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).session_manager(session).build()?;
    agent.invoke("First message").await?; // 2 messages, no snapshot
    agent.invoke("Second message").await?; // 4 messages, immutable snapshot created
    // --8<-- [end:snapshot_trigger]
    Ok(())
}

async fn save_latest_strategy_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:save_latest_strategy]
    use std::sync::Arc;

    use strands_agents::session::{FileStorage, SaveLatestStrategy, SessionManager};
    use strands_agents::session::SessionManagerOptions;

    let options = SessionManagerOptions {
        // default; also: Message | Trigger
        save_latest_on: SaveLatestStrategy::Invocation,
        ..Default::default()
    };
    let session = SessionManager::with_options(
        "my-session",
        Arc::new(FileStorage::new("./sessions/")),
        options,
    )?;
    // --8<-- [end:save_latest_strategy]
    let _ = session;
    Ok(())
}

async fn list_and_restore_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:list_and_restore]
    use std::sync::Arc;

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::session::{FileStorage, SessionManager};
    use strands_agents::Agent;

    let storage = Arc::new(FileStorage::new("./sessions/"));
    let session = SessionManager::new("my-session", storage)?;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .session_manager(session.clone())
        .build()?;
    agent.initialize().await?;

    // Force an immutable checkpoint, then list every snapshot id (chronological order)
    session.save_snapshot(&agent, false).await?;
    let snapshot_ids = session.list_snapshot_ids(&agent, None, None).await?;

    // Restore the agent to a specific checkpoint
    session.restore_snapshot(&agent, Some(&snapshot_ids[0])).await?;
    // --8<-- [end:list_and_restore]
    Ok(())
}

async fn delete_session_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:delete_session]
    use std::sync::Arc;

    use strands_agents::session::{FileStorage, SessionManager};

    let session = SessionManager::new("my-session", Arc::new(FileStorage::new("./sessions/")))?;

    // Remove all snapshots and manifests for this session
    session.delete_session().await?;
    // --8<-- [end:delete_session]
    Ok(())
}

// --8<-- [start:custom_storage]
use async_trait::async_trait;
use strands_agents::session::{Snapshot, SnapshotLocation, SnapshotManifest, SnapshotStorage};
use strands_agents::Result;

// Implement SnapshotStorage to plug in any backend
struct MyStorage;

#[async_trait]
impl SnapshotStorage for MyStorage {
    async fn save_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: &str,
        is_latest: bool,
        snapshot: &Snapshot,
    ) -> Result<()> {
        // Persist the snapshot
        Ok(())
    }

    async fn load_snapshot(
        &self,
        location: &SnapshotLocation,
        snapshot_id: Option<&str>,
    ) -> Result<Option<Snapshot>> {
        // Read it back; `None` means "nothing stored yet"
        Ok(None)
    }

    async fn list_snapshot_ids(
        &self,
        location: &SnapshotLocation,
        limit: Option<usize>,
        start_after: Option<&str>,
    ) -> Result<Vec<String>> {
        Ok(vec![])
    }

    async fn delete_session(&self, session_id: &str) -> Result<()> {
        Ok(())
    }

    async fn load_manifest(&self, location: &SnapshotLocation) -> Result<SnapshotManifest> {
        Ok(SnapshotManifest {
            schema_version: "1.0".to_owned(),
            updated_at: "1970-01-01T00:00:00.000Z".to_owned(),
        })
    }

    async fn save_manifest(
        &self,
        location: &SnapshotLocation,
        manifest: &SnapshotManifest,
    ) -> Result<()> {
        // Persist the manifest
        Ok(())
    }
}

// --8<-- [end:custom_storage]

async fn custom_storage_agent() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:custom_storage_usage]
    use std::sync::Arc;

    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::session::SessionManager;
    use strands_agents::Agent;

    let session = SessionManager::new("user-789", Arc::new(MyStorage))?;
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).session_manager(session).build()?;
    // --8<-- [end:custom_storage_usage]
    let _ = agent;
    Ok(())
}
