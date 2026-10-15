//! Snippets for `tools-and-instructions.mdx`.

async fn harness_instructions() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_instructions]
    use strands_harness::{create_harness, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions {
        instructions: Some("You are a support assistant. Always link the ticket.".into()),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_instructions]

    let _ = agent;
    Ok(())
}

async fn sdk_instructions() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_instructions]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .system_prompt("You are a support assistant. Always link the ticket.")
        .build()?;
    // --8<-- [end:sdk_instructions]

    let _ = agent;
    Ok(())
}

async fn register_tools() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:register_tools]
    use std::sync::Arc;

    use strands_agents::tool;
    use strands_harness::{create_harness, HarnessAgentOptions};

    /// Fetch a support ticket by id.
    #[tool]
    async fn get_ticket(
        /// The ticket id.
        ticket_id: String,
    ) -> String {
        format!("Ticket {ticket_id}: open, assigned to support.")
    }

    let agent = create_harness(HarnessAgentOptions {
        instructions: Some("You are a support assistant. Always link the ticket.".into()),
        tools: vec![Arc::new(GetTicketTool::new())],
        ..Default::default()
    })
    .await?;
    // --8<-- [end:register_tools]

    let _ = agent;
    Ok(())
}

async fn harness_builtin_tools() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_builtin_tools]
    use strands_harness::{create_harness, BuiltinToolsConfig, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions {
        // Just these two; an empty list for none.
        builtin_tools: Some(BuiltinToolsConfig::list(["read", "shell"])),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_builtin_tools]

    let _ = agent;
    Ok(())
}

async fn sdk_builtin_tools() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_builtin_tools]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, FileEditorTool, ShellTool};

    // The Strands Harness SDK has no built-in toggle; pass exactly the tools you want.
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(FileEditorTool::new())
        .tool(ShellTool::new())
        .build()?;
    // --8<-- [end:sdk_builtin_tools]

    let _ = agent;
    Ok(())
}
