//! Rust snippets for `agents-as-tools.mdx`.

async fn direct_passing_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:direct_passing]
    use std::sync::Arc;

    use strands_agents::agent::AgentAsToolOptions;
    use strands_agents::models::{BedrockModel, BedrockModelConfig, Model};
    use strands_agents::Agent;

    let model: Arc<dyn Model> =
        Arc::new(BedrockModel::new(BedrockModelConfig::default()).await?);

    // Create specialized agents
    let research_agent = Agent::builder()
        .model(Arc::clone(&model))
        .name("research_agent")
        .description(
            "Provides factual, well-sourced information in response to research \
             questions.",
        )
        .system_prompt(
            "You are a specialized research assistant. Focus only on providing \
             factual, well-sourced information in response to research questions. \
             Always cite your sources when possible.",
        )
        .build()?;

    let product_agent = Agent::builder()
        .model(Arc::clone(&model))
        .name("product_agent")
        .description(
            "Provides personalized product suggestions based on user preferences.",
        )
        .system_prompt(
            "You are a specialized product recommendation assistant. \
             Provide personalized product suggestions based on user preferences.",
        )
        .build()?;

    let travel_agent = Agent::builder()
        .model(Arc::clone(&model))
        .name("travel_agent")
        .description("Creates detailed travel itineraries based on user preferences.")
        .system_prompt(
            "You are a specialized travel planning assistant. \
             Create detailed travel itineraries based on user preferences.",
        )
        .build()?;

    // Create the orchestrator: each specialist keeps its own name and description
    let orchestrator = Agent::builder()
        .model(model)
        .system_prompt(
            "You are an assistant that routes queries to specialized agents:
- For research questions and factual information → Use the research_agent tool
- For product recommendations and shopping advice → Use the product_agent tool
- For travel planning and itineraries → Use the travel_agent tool
- For simple questions not requiring specialized knowledge → Answer directly

Always select the most appropriate tool based on the user's query.",
        )
        .tools([
            research_agent.as_tool(AgentAsToolOptions::default()),
            product_agent.as_tool(AgentAsToolOptions::default()),
            travel_agent.as_tool(AgentAsToolOptions::default()),
        ])
        .build()?;
    // --8<-- [end:direct_passing]
    let _ = orchestrator;
    Ok(())
}

async fn as_tool_customization_scope() -> Result<(), Box<dyn std::error::Error>> {
    use std::sync::Arc;

    use strands_agents::agent::AgentAsToolOptions;
    use strands_agents::models::{BedrockModel, BedrockModelConfig, Model};
    use strands_agents::Agent;

    let model: Arc<dyn Model> =
        Arc::new(BedrockModel::new(BedrockModelConfig::default()).await?);
    let research_agent = Agent::builder()
        .model(Arc::clone(&model))
        .system_prompt("You are a specialized research assistant.")
        .build()?;

    // --8<-- [start:as_tool_customization]
    let orchestrator = Agent::builder()
        .model(model)
        .system_prompt("You are an assistant that routes queries to specialized agents.")
        .tools([research_agent.as_tool(
            AgentAsToolOptions::named("research_assistant").with_description(
                "Process and respond to research-related queries requiring factual \
                 information.",
            ),
        )])
        .build()?;
    // --8<-- [end:as_tool_customization]
    let _ = orchestrator;
    Ok(())
}

// --8<-- [start:research_assistant]
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::{tool, Agent};

/// Process and respond to research-related queries requiring factual information.
#[tool]
async fn research_assistant(
    /// A research question requiring factual information.
    query: String,
) -> Result<String, strands_agents::Error> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let research_agent = Agent::builder()
        .model(model)
        .system_prompt(
            "You are a specialized research assistant. Focus only on providing \
             factual, well-sourced information in response to research questions. \
             Always cite your sources when possible.",
        )
        .build()?;

    Ok(research_agent.invoke(query).await?.text())
}
// --8<-- [end:research_assistant]

// --8<-- [start:multiple_specialists]
/// Handle product recommendation queries by suggesting appropriate products.
#[tool]
async fn product_recommendation_assistant(
    /// A product inquiry with user preferences.
    query: String,
) -> Result<String, strands_agents::Error> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let product_agent = Agent::builder()
        .model(model)
        .system_prompt(
            "You are a specialized product recommendation assistant. \
             Provide personalized product suggestions based on user preferences.",
        )
        .build()?;

    Ok(product_agent.invoke(query).await?.text())
}

/// Create travel itineraries and provide travel advice.
#[tool]
async fn trip_planning_assistant(
    /// A travel planning request with destination and preferences.
    query: String,
) -> Result<String, strands_agents::Error> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let travel_agent = Agent::builder()
        .model(model)
        .system_prompt(
            "You are a specialized travel planning assistant. \
             Create detailed travel itineraries based on user preferences.",
        )
        .build()?;

    Ok(travel_agent.invoke(query).await?.text())
}
// --8<-- [end:multiple_specialists]

async fn orchestrator_scope() -> Result<(), Box<dyn std::error::Error>> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;

    // --8<-- [start:orchestrator]
    let orchestrator = Agent::builder()
        .model(model)
        .system_prompt(
            "You are an assistant that routes queries to specialized agents:
- For research questions and factual information → Use the research_assistant tool
- For recommendations and advice → Use the product_recommendation_assistant tool
- For travel planning and itineraries → Use the trip_planning_assistant tool
- For simple questions not requiring specialized knowledge → Answer directly

Always select the most appropriate tool based on the user's query.",
        )
        .tool(ResearchAssistantTool::new())
        .tool(ProductRecommendationAssistantTool::new())
        .tool(TripPlanningAssistantTool::new())
        .build()?;
    // --8<-- [end:orchestrator]

    // --8<-- [start:usage]
    let response = orchestrator
        .invoke("I'm looking for hiking boots for a trip to Patagonia next month")
        .await?;
    // --8<-- [end:usage]
    let _ = response;
    Ok(())
}

async fn delegation_basic_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:delegation_basic]
    use std::sync::Arc;

    use strands_agents::agent::{AgentAsToolOptions, AgentDelegation};
    use strands_agents::models::Model;

    let model: Arc<dyn Model> =
        Arc::new(BedrockModel::new(BedrockModelConfig::default()).await?);

    let billing_agent = Agent::builder()
        .model(Arc::clone(&model))
        .name("billing_expert")
        .description("Answers billing questions: charges, refunds, and invoices.")
        .system_prompt("You handle billing questions with precision.")
        .build()?;

    // The plugin names the delegate's tool; its reply becomes the final answer
    let orchestrator = Agent::builder()
        .model(model)
        .system_prompt(
            "Route billing questions to billing_expert.\n\
             Answer general questions yourself.",
        )
        .tools([billing_agent.as_tool(AgentAsToolOptions::named("billing_expert"))])
        .plugin(AgentDelegation::new(["billing_expert"]))
        .build()?;

    let result = orchestrator.invoke("Why was I charged twice?").await?;
    // result contains the billing agent's response, unchanged
    // --8<-- [end:delegation_basic]
    let _ = result;
    Ok(())
}
