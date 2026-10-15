//! Snippets for `overview.mdx`.

async fn chat_server() {
    // --8<-- [start:chat_server]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, Result};

    // Your framework's `POST /chat` route calls this.
    async fn chat(prompt: String) -> Result<String> {
        // The agent lives in this process: no scheduler or database to reach.
        let agent = Agent::new(BedrockModel::default_model().await?)?;
        Ok(agent.invoke(prompt).await?.text())
    }
    // --8<-- [end:chat_server]
}

async fn provider_bedrock() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:provider_bedrock]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Amazon Bedrock is the default, so the default model config is all you need.
    let agent = Agent::new(BedrockModel::default_model().await?)?;
    let result = agent.invoke("What can you help me build?").await?;
    println!("{result}");
    // --8<-- [end:provider_bedrock]

    Ok(())
}

async fn provider_anthropic() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:provider_anthropic]
    use strands_agents::models::anthropic::{AnthropicModel, AnthropicModelConfig};
    use strands_agents::Agent;

    let model = AnthropicModel::new(AnthropicModelConfig {
        api_key: Some("<KEY>".into()),
        model_id: Some("claude-sonnet-5".into()),
        ..Default::default()
    })?;
    let agent = Agent::new(model)?;
    let result = agent.invoke("What can you help me build?").await?;
    println!("{result}");
    // --8<-- [end:provider_anthropic]

    Ok(())
}

async fn provider_openai() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:provider_openai]
    use strands_agents::models::openai::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let model = OpenAIModel::new(OpenAIModelConfig {
        api_key: Some("<KEY>".into()),
        model_id: Some("gpt-5.4".into()),
        ..Default::default()
    })?;
    let agent = Agent::new(model)?;
    let result = agent.invoke("What can you help me build?").await?;
    println!("{result}");
    // --8<-- [end:provider_openai]

    Ok(())
}
