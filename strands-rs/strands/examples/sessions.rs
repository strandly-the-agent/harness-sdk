//! Sessions: a `SessionManager` saves the agent's conversation and state to storage after each
//! invocation, and an agent built with the same session id picks up where the last one stopped.
//!
//! This example scripts the model with `strands::testing::MockMessageModel` and stores the
//! session in a temporary directory, so it runs offline:
//!
//! ```sh
//! cargo run -p strands --example sessions
//! ```

#![expect(clippy::print_stdout, reason = "examples print their results")]

use std::sync::Arc;

use strands::session::{SessionManager, SessionManagerConfig};
use strands::storage::{LocalFileStorage, Storage};
use strands::testing::MockMessageModel;
use strands::types::messages::TextBlock;
use strands::{Agent, Error};

fn agent(storage: Arc<dyn Storage>, reply: &str) -> Result<Agent, Error> {
    let model = MockMessageModel::new();
    model.add_turn(vec![TextBlock::new(reply).into()]);

    let session_manager = SessionManager::new(SessionManagerConfig {
        storage: Some(storage),
        session_id: Some("user-123".to_owned()),
        ..Default::default()
    })?;
    Agent::builder()
        .model(model)
        .session_manager(session_manager)
        .printer(false)
        .build()
}

#[tokio::main]
async fn main() -> Result<(), Error> {
    let dir = tempfile::tempdir().map_err(Error::other)?;
    let storage: Arc<dyn Storage> = Arc::new(LocalFileStorage::new(dir.path().display().to_string()));

    // The first agent talks, and the session manager saves a snapshot after the invocation.
    let mut first = agent(Arc::clone(&storage), "Nice to meet you, Ada.")?;
    first.app_state().set("visits", 1)?;
    first.invoke("Hi, my name is Ada.").await?;
    first.shutdown().await?;

    // A second agent with the same session id restores the conversation and the app state
    // before its first invocation.
    let mut second = agent(storage, "Your name is Ada.")?;
    let result = second.invoke("What is my name?").await?;
    println!("{result}");
    println!("messages in the conversation: {}", second.messages().len());
    println!("visits: {:?}", second.app_state().get::<u32>("visits")?);

    second.shutdown().await
}
