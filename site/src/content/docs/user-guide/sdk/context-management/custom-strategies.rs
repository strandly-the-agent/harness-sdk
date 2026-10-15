//! Rust snippets for `context-management/custom-strategies.mdx`.

use strands_agents::Result;

async fn basic_config() -> Result<()> {
    // --8<-- [start:basic_config]
    use std::sync::Arc;

    use strands_agents::context_manager::strategies::{
        Offload, OffloadConditions, OffloadTarget, TruncateConfig,
    };
    use strands_agents::models::BedrockModel;
    use strands_agents::{
        Agent, ContextManagerConfig, ContextManagerOption, StashOption,
    };

    let truncate =
        Offload::truncate(OffloadTarget::ToolResults, TruncateConfig::default())
            .when(OffloadConditions { threshold: Some(2000.0), ..Default::default() });
    let summarize = Offload::summarize(OffloadTarget::All).when(OffloadConditions {
        utilization: Some(0.8),
        preserve_recent: Some(4.0),
        ..Default::default()
    });

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .context_manager(ContextManagerOption::Config(ContextManagerConfig {
            strategies: Some(vec![Arc::new(truncate), Arc::new(summarize)]),
            stash: StashOption::Enabled,
        }))
        .build()?;
    // --8<-- [end:basic_config]
    let _ = agent;
    Ok(())
}

fn truncate() {
    // --8<-- [start:truncate]
    use strands_agents::context_manager::strategies::{
        Offload, OffloadConditions, OffloadTarget, TruncateConfig,
    };

    Offload::truncate(
        OffloadTarget::ToolResults,
        TruncateConfig { preview_tokens: Some(750), ..Default::default() },
    )
    .when(OffloadConditions { threshold: Some(1500.0), ..Default::default() });
    // --8<-- [end:truncate]
}

fn summarize() {
    // --8<-- [start:summarize]
    use strands_agents::context_manager::strategies::{
        Offload, OffloadConditions, OffloadTarget,
    };

    Offload::summarize(OffloadTarget::All).when(OffloadConditions {
        utilization: Some(0.85),
        preserve_recent: Some(4.0),
        ..Default::default()
    });
    // --8<-- [end:summarize]
}

fn drop_strategy() {
    // --8<-- [start:drop]
    use strands_agents::context_manager::strategies::{
        Offload, OffloadConditions, OffloadTarget,
    };

    Offload::drop(OffloadTarget::ToolResults)
        .when(OffloadConditions { preserve_recent: Some(5.0), ..Default::default() });
    // --8<-- [end:drop]
}

fn conditions() {
    use strands_agents::context_manager::strategies::{
        Offload, OffloadConditions, OffloadTarget, TruncateConfig,
    };

    // --8<-- [start:conditions_threshold]
    Offload::truncate(OffloadTarget::ToolResults, TruncateConfig::default())
        .when(OffloadConditions { threshold: Some(2000.0), ..Default::default() });
    // --8<-- [end:conditions_threshold]

    // --8<-- [start:conditions_utilization]
    Offload::summarize(OffloadTarget::All).when(OffloadConditions {
        utilization: Some(0.85),
        preserve_recent: Some(4.0),
        ..Default::default()
    });
    // --8<-- [end:conditions_utilization]

    // --8<-- [start:conditions_both]
    Offload::truncate(OffloadTarget::ToolResults, TruncateConfig::default()).when(
        OffloadConditions {
            threshold: Some(1500.0),
            utilization: Some(0.9),
            ..Default::default()
        },
    );
    // --8<-- [end:conditions_both]
}

async fn stash_disabled() -> Result<()> {
    // --8<-- [start:stash_disabled]
    use strands_agents::models::BedrockModel;
    use strands_agents::{
        Agent, ContextManagerConfig, ContextManagerOption, StashOption,
    };

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .context_manager(ContextManagerOption::Config(ContextManagerConfig {
            stash: StashOption::Disabled,
            ..Default::default()
        }))
        .build()?;
    // --8<-- [end:stash_disabled]
    let _ = agent;
    Ok(())
}

fn tool_targets() {
    // --8<-- [start:tool_targets]
    use strands_agents::context_manager::strategies::OffloadTarget;

    // Specific tools
    OffloadTarget::tools(["tool::shell", "tool::fileEditor"]);

    // Every tool except these
    OffloadTarget::tools(["!tool::shell"]);
    // --8<-- [end:tool_targets]
}
