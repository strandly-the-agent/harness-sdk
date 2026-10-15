//! Rust snippets for `streaming/index.mdx`.

async fn quick_example_async_iterator() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:quick_example_async_iterator]
    use futures::StreamExt;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let mut stream = agent.stream("Tell me about agents in one sentence.");
    while let Some(event) = stream.next().await {
        let event = event?;

        // Print out the model text delta event data
        if let Some(text) = event.text_delta() {
            print!("{text}");
        }
    }
    println!("\nDone!");
    // --8<-- [end:quick_example_async_iterator]

    Ok(())
}
