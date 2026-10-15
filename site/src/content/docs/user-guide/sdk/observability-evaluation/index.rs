//! Rust snippets for `observability-evaluation/index.mdx`.

async fn observe_agent() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:observe_agent]
    use strands_agents::models::BedrockModel;
    use strands_agents::telemetry::TelemetryConfig;
    use strands_agents::Agent;

    // Your application installs the exporter and the global tracer provider; every
    // model and tool call the agent makes is then captured as a span
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .telemetry(TelemetryConfig::from_env())
        .build()?;

    agent.invoke("What is agent observability?").await?;
    // --8<-- [end:observe_agent]

    Ok(())
}
