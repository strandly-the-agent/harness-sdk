//! Snippets for `model.mdx`.

async fn harness_model() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_model]
    use strands_harness::{create_harness, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions {
        model: Some("openai/gpt-5.6-sol".into()),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_model]

    let _ = agent;
    Ok(())
}

async fn sdk_model() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_model]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let model = BedrockModel::with_model_id("global.anthropic.claude-sonnet-5").await?;
    let agent = Agent::new(model)?;
    // --8<-- [end:sdk_model]

    let _ = agent;
    Ok(())
}

async fn model_instance() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:model_instance]
    use std::sync::Arc;

    use strands_agents::models::{BedrockModel, Model};
    use strands_harness::{create_harness, HarnessAgentOptions};

    let model: Arc<dyn Model> = Arc::new(
        BedrockModel::builder()
            .model_id("global.anthropic.claude-opus-5")
            .region("us-west-2")
            .build()
            .await?,
    );
    let agent = create_harness(HarnessAgentOptions {
        model: Some(model.into()),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:model_instance]

    let _ = agent;
    Ok(())
}

async fn harness_effort() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_effort]
    use strands_harness::{create_harness, Effort, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions {
        model: Some("anthropic/claude-opus-5".into()),
        effort: Effort::High,
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_effort]

    let _ = agent;
    Ok(())
}

async fn sdk_effort() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_effort]
    use serde_json::json;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".into()),
        additional_request_fields: Some(json!({
            "thinking": { "type": "enabled", "budget_tokens": 4096 }
        })),
        ..Default::default()
    })
    .await?;
    let agent = Agent::new(bedrock_model)?;
    // --8<-- [end:sdk_effort]

    let _ = agent;
    Ok(())
}
