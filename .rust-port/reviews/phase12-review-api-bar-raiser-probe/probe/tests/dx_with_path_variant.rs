use strands::{InterruptResponse, StopReason};
use strands_harness::{create_harness, Effort, HarnessAgentOptions, McpServersOption};

#[allow(dead_code)]
async fn dx(ask_user: fn(&strands::interrupt::Interrupt) -> InterruptResponse) -> Result<(), Box<dyn std::error::Error>> {
    let mut agent = create_harness(
        HarnessAgentOptions::builder()
            .model("anthropic/claude-opus-5")
            .effort(Effort::High)
            .mcp_servers(McpServersOption::Path("./mcp.json".into()))
            .interventions("ask")
            .build(),
    )
    .await?;

    let mut result = agent.invoke("Fix the failing test in src/lib.rs").await?;
    while result.stop_reason() == StopReason::Interrupt {
        let answers: Vec<InterruptResponse> = result.interrupts().iter().map(ask_user).collect();
        result = agent.invoke(answers).await?;
    }
    agent.shutdown().await?;
    Ok(())
}
