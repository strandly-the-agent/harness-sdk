//! Snippets for `async-iterators.mdx`.

async fn basic_usage() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:basic_usage]
    use futures::StreamExt;
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, SleepTool};

    // Initialize our agent without a printer
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(SleepTool::default())
        .build()?;

    // Iterate over streamed agent events
    let mut stream = agent.stream("Record that my favorite color is blue!");
    while let Some(event) = stream.next().await {
        println!("{}", event?.type_name());
    }
    // --8<-- [end:basic_usage]

    Ok(())
}

async fn ndjson_handler() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:ndjson_handler]
    use futures::StreamExt;
    use serde_json::json;
    use strands_agents::agent::AgentStreamEvent;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;
    use tokio::io::{AsyncWrite, AsyncWriteExt};

    /// Stream one prompt to a response body as newline-delimited JSON.
    async fn handle_stream_request(
        prompt: String,
        mut body: impl AsyncWrite + Unpin,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let agent = Agent::new(BedrockModel::default_model().await?)?;

        let mut stream = agent.stream(prompt);
        while let Some(event) = stream.next().await {
            // Events hold live handles, so pick the fields to forward yourself;
            // `type_name()` is the same discriminator the TypeScript events use.
            let event = event?;
            let line = match event.text_delta() {
                Some(text) => json!({ "type": event.type_name(), "data": text }),
                None => json!({ "type": event.type_name() }),
            };
            body.write_all(format!("{line}\n").as_bytes()).await?;
        }
        body.flush().await?;
        Ok(())
    }
    // --8<-- [end:ndjson_handler]

    handle_stream_request("Hello".to_owned(), tokio::io::sink()).await

}

// --8<-- [start:agent_loop_lifecycle]
use strands_agents::agent::AgentStreamEvent;
use strands_agents::models::ModelStreamEvent;
use strands_agents::models::ContentBlockStart;

fn process_event(event: &AgentStreamEvent) {
    // Track agent loop lifecycle
    match event {
        AgentStreamEvent::BeforeInvocation(_) => println!("Agent loop initialized"),
        AgentStreamEvent::BeforeModelCall(_) => println!("Agent loop cycle starting"),
        AgentStreamEvent::ModelMessage(e) => {
            println!("New message created: {:?}", e.message.role);
        }
        AgentStreamEvent::BeforeTools(_) => println!("About to execute tool!"),
        AgentStreamEvent::AfterTools(_) => println!("Finished executing tool!"),
        AgentStreamEvent::AfterInvocation(_) => println!("Agent loop completed"),
        _ => {}
    }

    // Track tool usage
    if let AgentStreamEvent::ModelStreamUpdate(update) = event {
        if let ModelStreamEvent::ContentBlockStart {
            start: Some(ContentBlockStart::ToolUse { name, .. }),
            ..
        } = &update.event
        {
            println!("\nUsing tool: {name}");
        }
    }
}
// --8<-- [end:agent_loop_lifecycle]

// --8<-- [start:sub_agent_basic]
use strands_agents::{tool, ToolContext};

/// Agent that calculates the answer to a math problem input.
#[tool]
async fn math_agent(
    ctx: ToolContext,
    input: String,
) -> Result<String, strands_agents::Error> {
    use futures::StreamExt;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let math_agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .system_prompt("You are a math expert. Answer a math problem in one sentence")
        .build()?;

    // Stream from the sub-agent, reporting each text delta as tool progress
    let mut answer = String::new();
    let mut deltas = Box::pin(math_agent.stream_text(input));
    while let Some(delta) = deltas.next().await {
        let delta = delta?;
        ctx.progress(delta.clone()).await;
        answer.push_str(&delta);
    }

    Ok(answer)
}

async fn sub_agent_basic() -> Result<(), Box<dyn std::error::Error>> {
    use futures::StreamExt;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(MathAgentTool::new())
        .build()?;

    let mut stream = agent.stream("What is 2 * 3? Use your tool.");
    while let Some(event) = stream.next().await {
        if let AgentStreamEvent::ToolStreamUpdate(update) = event? {
            println!("Tool Event: {:?}", update.event.data);
        }
    }
    println!("\nDone!");

    Ok(())
}
// --8<-- [end:sub_agent_basic]
