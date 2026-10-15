//! Snippets for `events.mdx`.

async fn event_serialization() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:event_serialization]
    use futures::StreamExt;
    use serde_json::json;
    use strands_agents::agent::AgentStreamEvent;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let mut stream = agent.stream("Hello");
    while let Some(event) = stream.next().await {
        let event = event?;

        // Forward text deltas for real-time display
        let payload = if let Some(text) = event.text_delta() {
            Some(json!({ "type": "text", "data": text }))
        // Forward tool usage for progress indicators
        } else if let AgentStreamEvent::BeforeToolCall(call) = &event {
            Some(json!({ "type": "tool", "name": call.tool_use.name }))
        // Forward the final result
        } else if let Some(result) = event.result() {
            Some(json!({ "type": "result", "stopReason": result.stop_reason }))
        // Skip everything else (lifecycle signals, raw deltas, reasoning, etc.)
        } else {
            None
        };

        if let Some(payload) = payload {
            println!("data: {payload}\n");
        }
    }
    // --8<-- [end:event_serialization]

    Ok(())
}
