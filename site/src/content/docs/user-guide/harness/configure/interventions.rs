//! Snippets for `interventions.mdx`.

async fn harness_ask() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_ask]
    use strands_harness::{create_harness, HarnessAgentOptions, InterventionsOption};

    // Approve every tool call.
    let agent = create_harness(HarnessAgentOptions {
        interventions: InterventionsOption::presets(["ask"]),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_ask]

    let _ = agent;
    Ok(())
}

async fn sdk_ask() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_ask]
    use std::sync::Arc;

    use strands_agents::interventions::vended::hitl::HumanInTheLoop;
    use strands_agents::models::BedrockModel;
    use strands_agents::types::{InterruptResponseContent, StopReason};
    use strands_agents::{Agent, ShellTool};

    // HumanInTheLoop gates every tool call for approval.
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(ShellTool::new())
        .interventions([Arc::new(HumanInTheLoop::new()) as _])
        .build()?;
    let mut result = agent.invoke("Delete the temp files").await?;
    if result.stop_reason == StopReason::Interrupt {
        let answer = InterruptResponseContent::new(&result.interrupts[0].id, "yes");
        result = agent.invoke(vec![answer]).await?;
    }
    // --8<-- [end:sdk_ask]

    let _ = result;
    Ok(())
}

async fn sdk_policy() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:sdk_policy]
    use std::sync::Arc;

    use strands_agents::interventions::vended::hitl::HumanInTheLoop;
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, ShellTool};

    // The Strands Harness SDK has no natural-language risk policy (a Strands harness
    // convenience). Gate tool calls with a HumanInTheLoop handler instead.
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(ShellTool::new())
        .interventions([Arc::new(HumanInTheLoop::new()) as _])
        .build()?;
    // --8<-- [end:sdk_policy]

    let _ = agent;
    Ok(())
}

async fn harness_cedar() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:harness_cedar]
    use std::path::PathBuf;
    use std::sync::Arc;

    use strands_agents::interventions::vended::CedarAuthorization;
    use strands_harness::{
        create_harness, HarnessAgentOptions, InterventionValue, InterventionsOption,
    };

    // The Rust harness does not load a `.cedar` path for you; build the handler and
    // pass it.
    let cedar =
        CedarAuthorization::from_policies(PathBuf::from("./policies/agent.cedar"))?;
    let agent = create_harness(HarnessAgentOptions {
        interventions: InterventionsOption::Values(vec![InterventionValue::Handler(
            Arc::new(cedar),
        )]),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:harness_cedar]

    let _ = agent;
    Ok(())
}
