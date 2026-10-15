//! Rust snippets for `observability-evaluation/traces.mdx`.

async fn code_configuration_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:code_configuration_agent]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::telemetry::TelemetryConfig;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;

    // Reads OTEL_SEMCONV_STABILITY_OPT_IN and detects Langfuse endpoints
    let agent =
        Agent::builder().model(model).telemetry(TelemetryConfig::from_env()).build()?;

    // Use agent normally
    let result = agent.invoke("What can you help me with?").await?;
    println!("{}", result.last_message.text());
    // --8<-- [end:code_configuration_agent]
    Ok(())
}

async fn custom_attributes_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:custom_attributes]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::telemetry::AttrValue;
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;

    // Merged onto every span this agent emits
    let agent = Agent::builder()
        .model(model)
        .trace_attributes([
            ("session.id".to_string(), AttrValue::from("abc-1234")),
            ("user.id".to_string(), AttrValue::from("user-email-example@domain.com")),
            ("tags".to_string(), AttrValue::from("Agent-SDK")),
        ])
        .build()?;

    agent.invoke("Hello").await?;
    // --8<-- [end:custom_attributes]
    Ok(())
}

async fn custom_spans_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:custom_spans]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::telemetry::opentelemetry::global;
    use strands_agents::telemetry::opentelemetry::trace::{Span, Tracer};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    // The SDK's spans and yours share the global tracer provider
    let tracer = global::tracer("my-application");
    let mut span = tracer.start("user-request-processing");

    let result = agent.invoke("Tell me about Mars").await?;
    println!("{}", result.last_message.text());

    span.end();
    // --8<-- [end:custom_spans]
    Ok(())
}

async fn end_to_end_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:end_to_end]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::telemetry::TelemetryConfig;
    use strands_agents::Agent;

    // Your application installs the exporter and the global tracer provider first
    let model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".to_string()),
        ..Default::default()
    })
    .await?;

    let agent = Agent::builder()
        .model(model)
        .system_prompt("You are a helpful AI assistant")
        .telemetry(TelemetryConfig::from_env())
        .build()?;

    // Execute a series of interactions that will be traced
    let result = agent
        .invoke("Find me information about Mars. What is its atmosphere like?")
        .await?;
    println!("{}", result.last_message.text());

    // Ask a follow-up that uses tools
    let result = agent
        .invoke("How long would a trip from Earth to Mars take at 100,000 km/h?")
        .await?;
    println!("{}", result.last_message.text());

    // Each interaction creates a complete trace, plus a local tree in result.traces
    // --8<-- [end:end_to_end]
    Ok(())
}
