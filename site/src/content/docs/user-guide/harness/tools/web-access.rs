//! Snippets for `web-access.mdx`.

async fn sdk_web_fetch() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_web_fetch]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, HttpRequestTool};

    // No web_fetch or summarizer model; HttpRequestTool returns raw HTTP
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(HttpRequestTool::new())
        .build()?;
    // --8<-- [end:sdk_web_fetch]

    let _ = agent;
    Ok(())
}
