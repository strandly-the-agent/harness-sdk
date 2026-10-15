//! Snippets for `prompts.mdx`.

async fn system_prompt() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:system_prompt]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .system_prompt(
            "You are a financial advisor specialized in retirement planning. \
             Use tools to gather information and provide personalized advice. \
             Always explain your reasoning and cite sources when possible.",
        )
        .build()?;
    // --8<-- [end:system_prompt]

    let _ = agent;
    Ok(())
}

async fn text_prompt() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // --8<-- [start:text_prompt]
    let response = agent.invoke("What is the time in Seattle").await?;
    // --8<-- [end:text_prompt]

    let _ = response;
    Ok(())
}

async fn multimodal_prompt() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // --8<-- [start:multimodal_prompt]
    use strands_agents::types::{ImageBlock, ImageFormat, ImageSource};
    use strands_agents::ContentBlock;

    let image_bytes = std::fs::read("path/to/image.png")?;

    let response = agent
        .invoke(vec![
            ContentBlock::text("What can you see in this image?"),
            ContentBlock::Image(ImageBlock {
                format: ImageFormat::Png,
                source: ImageSource::Bytes(image_bytes),
            }),
        ])
        .await?;
    // --8<-- [end:multimodal_prompt]

    let _ = response;
    Ok(())
}

/// Current time in a timezone.
#[strands_agents::tool]
async fn current_time(
    /// IANA timezone name, for example `US/Pacific`.
    timezone: String,
) -> String {
    format!("the time in {timezone}")
}

async fn direct_tool_calls() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(CurrentTimeTool::new())
        .build()?;

    // --8<-- [start:direct_tool_calls]
    use serde_json::json;

    let result =
        agent.tool("current_time")?.call(json!({ "timezone": "US/Pacific" })).await?;
    // --8<-- [end:direct_tool_calls]

    let _ = result;
    Ok(())
}
