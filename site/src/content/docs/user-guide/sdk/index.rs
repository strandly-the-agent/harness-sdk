//! Snippets for `index.mdx`.

async fn minimal_agent() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:minimal_agent]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;
    let result = agent.invoke("Explain the agent loop in one sentence.").await?;
    println!("{result}");
    // --8<-- [end:minimal_agent]

    Ok(())
}
