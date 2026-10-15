//! Rust snippets for `safety-security/index.mdx`.

async fn guardrail_agent() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:guardrail_agent]
    use strands_agents::models::bedrock::BedrockGuardrailConfig;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    // The guardrail screens every prompt and response; blocked content is redacted
    let model = BedrockModel::new(BedrockModelConfig {
        guardrail_config: Some(BedrockGuardrailConfig {
            guardrail_identifier: "your-guardrail-id".to_string(),
            guardrail_version: "1".to_string(),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;

    let agent = Agent::new(model)?;
    agent.invoke("Summarize our refund policy for a customer.").await?;
    // --8<-- [end:guardrail_agent]

    Ok(())
}
