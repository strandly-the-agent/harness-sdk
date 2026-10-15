//! Snippets for `composing-with-sdk.mdx`.

use async_trait::async_trait;
use strands_agents::plugins::Plugin;
use strands_agents::{Agent, Result};

/// Your own `Plugin` implementation.
struct MyMetricsPlugin;

#[async_trait]
impl Plugin for MyMetricsPlugin {
    fn name(&self) -> &str {
        "my-metrics"
    }

    async fn init_agent(&self, _agent: &Agent) -> Result<()> {
        Ok(())
    }
}

async fn compose_sdk_features() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:compose_sdk_features]
    use std::sync::Arc;

    use strands_agents::context_manager::ContextManagerOption;
    use strands_agents::interventions::vended::hitl::HumanInTheLoop;
    use strands_harness::{
        create_harness, HarnessAgentOptions, InterventionValue, InterventionsOption,
    };

    let agent = create_harness(HarnessAgentOptions {
        context_manager: ContextManagerOption::Agentic,
        // Runs next to Strands harness's built-in plugins.
        plugins: vec![Arc::new(MyMetricsPlugin)],
        // A Strands Harness SDK feature, wired straight through.
        interventions: InterventionsOption::Values(vec![InterventionValue::Handler(
            Arc::new(HumanInTheLoop::new()),
        )]),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:compose_sdk_features]

    let _ = agent;
    Ok(())
}

async fn modify_after_construction() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:modify_after_construction]
    use strands_harness::{create_harness, HarnessAgentOptions};

    let agent = create_harness(HarnessAgentOptions::default()).await?;
    let prompt = agent.system_prompt().map(|p| p.text()).unwrap_or_default();
    agent.set_system_prompt(Some(
        format!("{prompt}\n\nAlways cite file paths as file:line.").into(),
    ));
    // --8<-- [end:modify_after_construction]

    Ok(())
}

async fn borrow_the_prompt() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:borrow_the_prompt]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;
    use strands_harness::build_system_prompt;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .system_prompt(build_system_prompt(Some("You are a migration assistant."), &[]))
        .build()?;
    // --8<-- [end:borrow_the_prompt]

    let _ = agent;
    Ok(())
}
