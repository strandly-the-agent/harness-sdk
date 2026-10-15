//! Rust snippets for `steering.mdx`.

use strands_agents::tool;

/// Send an email to a recipient.
#[tool]
async fn send_email(recipient: String, subject: String, message: String) -> String {
    format!("Email to {recipient} sent: {subject} / {message}")
}

/// Search the web for a query.
#[tool]
async fn search_web(query: String) -> String {
    format!("Results for {query}")
}

// =====================
// Steering with natural language
// =====================

async fn llm_steering_example() -> std::result::Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:llm_steering]
    use strands_agents::interventions::vended::steering::LlmSteeringHandler;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let handler = LlmSteeringHandler::new(
        "You are providing guidance to ensure the agent follows best practices:\n\
         \n\
         Rules:\n\
         - Emails must always include a clear subject line\n\
         - Never send emails with aggressive or unprofessional language\n\
         - If the same tool has failed twice in a row, suggest a different approach\n\
         - Require human confirmation before sending emails to external domains",
    );

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(SendEmailTool::new())
        .tool(SearchWebTool::new())
        .intervention(handler)
        .build()?;

    let result = agent.invoke("Email the client about the project delay").await?;
    // The judge model evaluates tone and content → may guide the agent to soften
    // the language before sending
    println!("{result}");
    // --8<-- [end:llm_steering]
    Ok(())
}
