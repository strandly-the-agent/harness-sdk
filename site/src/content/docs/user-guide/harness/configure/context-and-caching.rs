//! Snippets for `context-and-caching.mdx`.

async fn harness_context_manager() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_context_manager]
    use strands_agents::context_manager::ContextManagerOption;
    use strands_harness::{create_harness, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions {
        context_manager: ContextManagerOption::Agentic,
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_context_manager]

    let _ = agent;
    Ok(())
}

async fn sdk_context_manager() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_context_manager]
    use std::sync::Arc;

    use strands_agents::context_manager::strategies::{Offload, OffloadConditions, OffloadTarget};
    use strands_agents::context_manager::{
        ContextManagerConfig, ContextManagerOption, StashOption, TruncateConfig,
    };
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // The Rust SDK has no conversation manager; context management is a strategy pipeline.
    let context_manager = ContextManagerOption::Config(ContextManagerConfig {
        strategies: Some(vec![Arc::new(
            Offload::truncate(OffloadTarget::ToolResults, TruncateConfig::default()).when(
                OffloadConditions {
                    threshold: Some(2_000.0),
                    ..Default::default()
                },
            ),
        )]),
        stash: StashOption::Enabled,
    });
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .context_manager(context_manager)
        .build()?;
    // --8<-- [end:sdk_context_manager]

    let _ = agent;
    Ok(())
}

async fn harness_caching_off() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_caching_off]
    use strands_harness::{create_harness, CachingOption, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions {
        caching: CachingOption::Off,
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_caching_off]

    let _ = agent;
    Ok(())
}

async fn sdk_caching() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_caching]
    use strands_agents::models::{BedrockModel, BedrockModelConfig, CacheConfig};
    use strands_agents::Agent;

    let bedrock_model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("global.anthropic.claude-sonnet-5".into()),
        cache_config: Some(CacheConfig {
            ttl: Some("1h".into()),
            ..Default::default()
        }),
        ..Default::default()
    })
    .await?;
    let agent = Agent::new(bedrock_model)?;
    // --8<-- [end:sdk_caching]

    let _ = agent;
    Ok(())
}
