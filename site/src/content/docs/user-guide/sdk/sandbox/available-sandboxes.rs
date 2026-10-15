//! Rust snippets for `sandbox/available-sandboxes.mdx`.

use strands_agents::Result;

async fn docker_constructor() -> Result<()> {
    // --8<-- [start:docker_constructor]
    use strands_agents::models::BedrockModel;
    use strands_agents::sandbox::DockerSandbox;
    use strands_agents::Agent;

    let sandbox = DockerSandbox::new("agent-workspace")
        .with_working_dir("/workspace")
        .with_user("1000:1000");

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .sandbox(sandbox)
        .build()?;
    agent.invoke("Run the test suite and summarize any failures").await?;
    // --8<-- [end:docker_constructor]
    Ok(())
}

async fn ssh_constructor() -> Result<()> {
    // --8<-- [start:ssh_constructor]
    use strands_agents::models::BedrockModel;
    use strands_agents::sandbox::SshSandbox;
    use strands_agents::Agent;

    let sandbox = SshSandbox::new("ubuntu@10.0.1.5", "/home/ubuntu/workspace")
        .with_identity_file("~/.ssh/agent_key");

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .sandbox(sandbox)
        .build()?;
    agent.invoke("Check disk usage and list running processes").await?;
    // --8<-- [end:ssh_constructor]
    Ok(())
}

async fn programmatic_access() -> Result<()> {
    // --8<-- [start:programmatic_access]
    use std::sync::Arc;

    use strands_agents::models::BedrockModel;
    use strands_agents::sandbox::{DockerSandbox, ExecuteOptions, Sandbox};
    use strands_agents::Agent;

    let sandbox = Arc::new(DockerSandbox::new("my-container-id"));
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .sandbox(Arc::clone(&sandbox))
        .build()?;

    // Seed an input file, let the agent work, then read the result back
    sandbox
        .write_text("/workspace/input.csv", "id,value\n1,42\n")
        .await?;

    agent
        .invoke("Summarize /workspace/input.csv into /workspace/out.txt")
        .await?;

    let result = sandbox
        .execute("cat /workspace/out.txt", ExecuteOptions::default())
        .await?;
    println!("{} {}", result.exit_code, result.stdout);
    // --8<-- [end:programmatic_access]
    Ok(())
}

async fn streaming() -> Result<()> {
    // --8<-- [start:streaming]
    use futures::StreamExt;
    use strands_agents::sandbox::{DockerSandbox, ExecuteOptions, Sandbox, SandboxEvent};

    let sandbox = DockerSandbox::new("my-container-id");

    let options = ExecuteOptions::default();
    let mut events = sandbox.execute_streaming("npm run build", options);
    while let Some(event) = events.next().await {
        match event? {
            SandboxEvent::Chunk(chunk) => print!("{}", chunk.data),
            SandboxEvent::Result(result) => println!("\nexit code: {}", result.exit_code),
        }
    }
    // --8<-- [end:streaming]
    Ok(())
}
