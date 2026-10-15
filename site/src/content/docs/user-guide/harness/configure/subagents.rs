//! Snippets for `subagents.mdx`.

async fn specialist_agent() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:specialist_agent]
    use strands_agents::agent::AgentAsToolOptions;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;
    use strands_harness::{create_harness, HarnessAgentOptions};

    let researcher = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .name("researcher")
        .description("Researches a topic and returns a concise, sourced summary.")
        .system_prompt("Research the given topic and return a concise, sourced summary.")
        .build()?;
    let agent = create_harness(HarnessAgentOptions {
        tools: vec![researcher.as_tool(AgentAsToolOptions::default())],
        ..Default::default()
    })
    .await?;
    // --8<-- [end:specialist_agent]

    let _ = agent;
    Ok(())
}

async fn harness_subagent_off() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_subagent_off]
    use strands_harness::{create_harness, BuiltinToolsConfig, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions {
        // No generalist delegate.
        builtin_tools: Some(BuiltinToolsConfig::mapping([("subagent", false.into())])),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_subagent_off]

    let _ = agent;
    Ok(())
}

async fn sdk_subagent_off() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_subagent_off]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // The generalist is Strands harness-specific; raw Strands Harness SDK has none to disable.
    let agent = Agent::new(BedrockModel::default_model().await?)?;
    // --8<-- [end:sdk_subagent_off]

    let _ = agent;
    Ok(())
}
