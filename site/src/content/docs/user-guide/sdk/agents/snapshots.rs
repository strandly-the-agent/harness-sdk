//! Rust snippets for `snapshots.mdx`. Each function is a scoping wrapper, never called.

async fn take_snapshot_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:take_snapshot]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("You are a helpful assistant")
        .build()?;
    agent.invoke("Hello!").await?;
    agent.state().set("user_id", "user-123");

    // Capture a snapshot with the session preset
    let snapshot = agent.take_snapshot(Default::default());

    println!("{}", snapshot.schema_version); // "1.0"
    println!("{}", snapshot.created_at); // ISO 8601 timestamp
    // messages, state, systemPrompt, modelState, interrupts
    println!("{:?}", snapshot.data.keys().collect::<Vec<_>>());
    // --8<-- [end:take_snapshot]
    Ok(())
}

async fn load_snapshot_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:load_snapshot]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("You are a helpful assistant")
        .build()?;
    agent.invoke("Hello!").await?;

    // Take a snapshot
    let snapshot = agent.take_snapshot(Default::default());

    // Continue the conversation
    agent.invoke("Tell me a joke").await?;
    agent.invoke("Tell me another one").await?;

    // Restore to the earlier state
    agent.load_snapshot(&snapshot)?;

    // The agent is back to the state after "Hello!"
    println!("{}", agent.messages().len()); // Only the messages from before the jokes
    // --8<-- [end:load_snapshot]
    Ok(())
}

async fn field_selection_example() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    // --8<-- [start:field_selection]
    use strands_agents::agent::{SnapshotField, SnapshotPreset, TakeSnapshotOptions};

    // Capture only messages and state: the session preset minus everything else
    let messages_only = agent.take_snapshot(
        TakeSnapshotOptions::default()
            .preset(SnapshotPreset::Session)
            .exclude(SnapshotField::SystemPrompt)
            .exclude(SnapshotField::ModelState)
            .exclude(SnapshotField::Interrupts),
    );

    // Session preset minus systemPrompt
    let options = TakeSnapshotOptions::default().exclude(SnapshotField::SystemPrompt);
    let no_prompt = agent.take_snapshot(options);
    // --8<-- [end:field_selection]
    let _ = (messages_only, no_prompt);
    Ok(())
}

async fn app_data_example() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    // --8<-- [start:app_data]
    use serde_json::json;
    use strands_agents::agent::TakeSnapshotOptions;

    let app_data = json!({
        "snapshotLabel": "After onboarding",
        "workflowStep": 3,
        "userDisplayName": "Alice",
    });
    let app_data = app_data.as_object().cloned().unwrap_or_default();
    let options = TakeSnapshotOptions::default().app_data(app_data);
    let snapshot = agent.take_snapshot(options);

    // Access app data later
    println!("{}", snapshot.app_data["snapshotLabel"]); // "After onboarding"
    println!("{}", snapshot.app_data["userDisplayName"]); // "Alice"
    // --8<-- [end:app_data]
    Ok(())
}

async fn serialization_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:serialization]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::{Agent, Snapshot};

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;
    agent.invoke("Hello!").await?;

    // Take a snapshot
    let snapshot = agent.take_snapshot(Default::default());

    // Serialize to a JSON string
    let json_string = serde_json::to_string(&snapshot)?;

    // Store to file, database, S3, etc.
    // ...

    // Later, restore from JSON
    let parsed: Snapshot = serde_json::from_str(&json_string)?;

    // Load into a new agent
    let new_model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let new_agent = Agent::builder().model(new_model).build()?;
    new_agent.load_snapshot(&parsed)?;
    // --8<-- [end:serialization]
    Ok(())
}

async fn checkpointing_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:checkpointing]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("You are a research assistant")
        .build()?;

    // Step 1: Gather information
    agent.invoke("Research the latest trends in AI agents").await?;
    let checkpoint1 = agent.take_snapshot(Default::default());

    // Step 2: Analyze (might fail or produce poor results)
    agent.invoke("Analyze the key themes and summarize").await?;
    let checkpoint2 = agent.take_snapshot(Default::default());

    // If step 2 didn't go well, roll back to checkpoint 1
    agent.load_snapshot(&checkpoint1)?;
    agent
        .invoke("Focus specifically on multi-agent systems and summarize")
        .await?;
    // --8<-- [end:checkpointing]
    let _ = checkpoint2;
    Ok(())
}

async fn branching_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:branching]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("You are a creative writer")
        .build()?;
    agent.invoke("Write the opening paragraph of a mystery novel").await?;

    // Save the branch point
    let branch_point = agent.take_snapshot(Default::default());

    // Branch A: formal tone
    agent.invoke("Continue in a formal, academic tone").await?;
    let formal_snapshot = agent.take_snapshot(Default::default());

    // Branch B: go back and try casual tone
    agent.load_snapshot(&branch_point)?;
    agent.invoke("Continue in a casual, conversational tone").await?;
    let casual_snapshot = agent.take_snapshot(Default::default());
    // --8<-- [end:branching]
    let _ = (formal_snapshot, casual_snapshot);
    Ok(())
}
