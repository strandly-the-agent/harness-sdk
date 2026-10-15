//! Rust examples for the model providers overview documentation.
//! These examples demonstrate model interchangeability.

use strands_agents::Error;

async fn basic_usage() -> Result<(), Error> {
    // --8<-- [start:basic_usage]
    use strands_agents::models::{
        BedrockModel, OpenAIApi, OpenAIModel, OpenAIModelConfig,
    };
    use strands_agents::Agent;

    // Use Bedrock
    let bedrock_model = BedrockModel::default_model().await?;
    let mut agent = Agent::new(bedrock_model)?;
    println!("{}", agent.invoke("What can you help me with?").await?);

    // Alternatively, use OpenAI by just switching model provider
    let openai_model = OpenAIModel::new(OpenAIModelConfig {
        api: OpenAIApi::Chat,
        model_id: Some("gpt-5.4".to_owned()),
        ..Default::default()
    })?;
    agent = Agent::new(openai_model)?;
    println!("{}", agent.invoke("What can you help me with?").await?);
    // --8<-- [end:basic_usage]
    Ok(())
}

async fn aux_model() -> Result<(), Error> {
    // --8<-- [start:aux_model]
    use strands_agents::context_manager::ContextManagerOption;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let opus = "global.anthropic.claude-opus-5-20260301-v1:0";
    let haiku = "global.anthropic.claude-haiku-4-5-20251001-v1:0";

    let agent = Agent::builder()
        .model(BedrockModel::with_model_id(opus).await?)
        .aux_model(BedrockModel::with_model_id(haiku).await?)
        .context_manager(ContextManagerOption::Auto)
        .build()?;
    // --8<-- [end:aux_model]
    let _ = agent;
    Ok(())
}
