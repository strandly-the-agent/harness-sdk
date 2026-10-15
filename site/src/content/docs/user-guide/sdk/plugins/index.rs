//! Rust snippets for `plugins/index.mdx`.

async fn hub_agent() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:hub_agent]
    use std::time::{SystemTime, UNIX_EPOCH};

    use strands_agents::models::BedrockModel;
    use strands_agents::plugins::ContextInjector;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .plugin(ContextInjector::new(|_context| {
            Box::pin(async {
                let now =
                    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
                Ok(Some(format!("<now>{}</now>", now.as_secs())))
            })
        }))
        .build()?;

    agent.invoke("What time is it right now?").await?;
    // --8<-- [end:hub_agent]

    Ok(())
}
