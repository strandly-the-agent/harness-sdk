//! Snippets for `sessions.mdx`.

async fn harness_session_id() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_session_id]
    use strands_harness::{create_harness, HarnessAgentOptions, SessionConfig, SessionOption};

    fn session() -> SessionOption {
        SessionOption::Config(SessionConfig {
            id: Some("refactor-parser".into()),
            ..Default::default()
        })
    }

    let agent = create_harness(HarnessAgentOptions {
        session: session(),
        ..Default::default()
    })
    .await?;
    agent
        .invoke("Let's refactor the parser. Where should we start?")
        .await?;
    // ...later, in a new process...
    let resumed = create_harness(HarnessAgentOptions {
        session: session(),
        ..Default::default()
    })
    .await?;
    resumed.invoke("Where did we leave off?").await?;
    // --8<-- [end:harness_session_id]

    Ok(())
}

async fn sdk_session_id() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_session_id]
    use std::sync::Arc;

    use strands_agents::models::BedrockModel;
    use strands_agents::session::{FileStorage, SessionManager};
    use strands_agents::Agent;

    let storage = Arc::new(FileStorage::new("./sessions"));
    let session = SessionManager::new("refactor-parser", storage.clone())?;
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .session_manager(session)
        .build()?;
    agent
        .invoke("Let's refactor the parser. Where should we start?")
        .await?;
    // ...later, in a new process...
    let resumed = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .session_manager(SessionManager::new("refactor-parser", storage)?)
        .build()?;
    resumed.invoke("Where did we leave off?").await?;
    // --8<-- [end:sdk_session_id]

    Ok(())
}

async fn harness_session_dir() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_session_dir]
    use strands_harness::{create_harness, HarnessAgentOptions, SessionConfig, SessionOption};

    let agent = create_harness(HarnessAgentOptions {
        session: SessionOption::Config(SessionConfig {
            id: Some("refactor-parser".into()),
            dir: Some("/var/lib/agent/sessions".into()),
        }),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_session_dir]

    let _ = agent;
    Ok(())
}

async fn sdk_session_dir() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_session_dir]
    use std::sync::Arc;

    use strands_agents::models::BedrockModel;
    use strands_agents::session::{FileStorage, SessionManager};
    use strands_agents::Agent;

    let storage = Arc::new(FileStorage::new("/var/lib/agent/sessions"));
    let session = SessionManager::new("refactor-parser", storage)?;
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .session_manager(session)
        .build()?;
    // --8<-- [end:sdk_session_dir]

    let _ = agent;
    Ok(())
}
