//! Rust snippets for `observability-evaluation/metrics.mdx`.

async fn basic_metrics_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:basic_metrics]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    let result = agent.invoke("What is the square root of 144?").await?;

    // Access metrics through the AgentResult
    let metrics = &result.metrics;
    println!("Total tokens: {}", metrics.accumulated_usage.total_tokens);
    println!("Total duration: {:?}ms", metrics.total_duration);
    println!("Tools used: {:?}", metrics.tool_metrics.keys());

    // Cache metrics (when available)
    if let Some(read) = metrics.accumulated_usage.cache_read_input_tokens {
        println!("Cache read tokens: {read}");
    }
    if let Some(write) = metrics.accumulated_usage.cache_write_input_tokens {
        println!("Cache write tokens: {write}");
    }
    // --8<-- [end:basic_metrics]
    Ok(())
}

async fn agent_loop_metrics_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:agent_loop_metrics]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    // First invocation
    let _result1 = agent.invoke("What is 5 + 3?").await?;

    // Second invocation
    let result2 = agent.invoke("What is the square root of 144?").await?;

    let metrics = &result2.metrics;

    // Access metrics for the latest invocation
    if let Some(latest) = metrics.agent_invocations.last() {
        println!("Invocation usage: {:?}", latest.usage);
        for cycle in &latest.cycles {
            println!("  Cycle {}: {:?}", cycle.cycle_id, cycle.usage);
        }
    }

    // Access all invocations
    for invocation in &metrics.agent_invocations {
        println!("Invocation usage: {:?}", invocation.usage);
        for cycle in &invocation.cycles {
            println!("  Cycle {}: {:?}", cycle.cycle_id, cycle.usage);
        }
    }

    // Computed metrics
    println!("Cycle count: {}", metrics.cycle_count);
    println!("Total duration: {:?}ms", metrics.total_duration);
    // --8<-- [end:agent_loop_metrics]
    Ok(())
}

async fn tool_metrics_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:tool_metrics]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    let result = agent.invoke("Summarize my notes").await?;

    for (name, tool) in &result.metrics.tool_metrics {
        let average_time = tool.total_time as f64 / tool.call_count.max(1) as f64;
        let success_rate = tool.success_count as f64 / tool.call_count.max(1) as f64;
        let calls = tool.call_count;
        println!("{name}: {calls} calls, {average_time}ms avg, {success_rate} success");
    }
    // --8<-- [end:tool_metrics]
    Ok(())
}

async fn metrics_summary_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:metrics_summary]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    let result = agent.invoke("What is the square root of 144?").await?;

    // Serialize metrics to JSON (camelCase keys, same shape as the other SDKs)
    println!("{:#}", result.metrics.to_json());
    // --8<-- [end:metrics_summary]
    Ok(())
}

async fn local_traces_example() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:local_traces]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).build()?;

    let result = agent.invoke("What is 15 * 8 + 42?").await?;

    // Access traces directly from the result
    for trace in &result.traces {
        let children = trace.children.len();
        println!("{} took {}ms ({children} children)", trace.name, trace.duration);
    }
    // --8<-- [end:local_traces]
    Ok(())
}
