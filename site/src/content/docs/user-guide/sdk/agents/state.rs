//! Snippets for `state.mdx`.

async fn conversation_history() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:conversation_history]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Create an agent
    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // Send a message and get a response
    agent.invoke("Hello!").await?;

    // Access the conversation history
    println!("{:?}", agent.messages()); // Shows all messages exchanged so far
    // --8<-- [end:conversation_history]

    Ok(())
}

async fn message_initialization() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:message_initialization]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, Message};

    // Create an agent with initial messages
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .messages([
            Message::user("Hello, my name is Strands!"),
            Message::assistant("Hi there! How can I help you today?"),
        ])
        .build()?;

    // Continue the conversation
    agent.invoke("What's my name?").await?;
    // --8<-- [end:message_initialization]

    Ok(())
}

/// Append a note to the notebook.
#[strands_agents::tool]
async fn notebook(
    /// What to do: `list` or `append`.
    mode: String,
) -> String {
    format!("notebook: {mode}")
}

async fn direct_tool_calling() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:direct_tool_calling]
    use serde_json::json;
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, CallOptions};

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(NotebookTool::new())
        .build()?;

    // Direct tool call with recording (default behavior)
    agent.tool("notebook")?.call(json!({ "mode": "list" })).await?;
    let recorded_message_count = agent.messages().len();

    // Direct tool call without recording
    agent
        .tool("notebook")?
        .call_with(
            json!({ "mode": "list" }),
            CallOptions::default().with_record_direct_tool_call(false),
        )
        .await?;

    println!("{}", recorded_message_count > 0); // true
    println!("{}", agent.messages().len() == recorded_message_count); // true
    // --8<-- [end:direct_tool_calling]

    Ok(())
}

async fn agent_state_basic() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:agent_state_basic]
    use serde_json::json;
    use strands_agents::models::BedrockModel;
    use strands_agents::state_store::StateStore;
    use strands_agents::Agent;

    // Create an agent with initial state
    let state = StateStore::new();
    state.set("user_preferences", json!({ "theme": "dark" }));
    state.set("session_count", 0);

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .state(state)
        .build()?;

    // Access state values
    let theme = agent.state().get("user_preferences");
    println!("{theme:?}"); // Some({"theme": "dark"})

    // Set new state values
    agent.state().set("last_action", "login");
    agent.state().set("session_count", 1);

    // Get state values individually
    println!("{:?}", agent.state().get("user_preferences"));
    println!("{:?}", agent.state().get("session_count"));

    // Delete state values
    agent.state().delete("last_action");
    // --8<-- [end:agent_state_basic]

    Ok(())
}

async fn state_validation() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:state_validation]
    use serde_json::json;
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // Values are JSON by construction: `set` takes anything that converts
    // into a `JsonValue`, so a non-serializable value is a compile error
    // rather than a runtime one.
    agent.state().set("string_value", "hello");
    agent.state().set("number_value", 42);
    agent.state().set("boolean_value", true);
    agent.state().set("list_value", json!([1, 2, 3]));
    agent.state().set("dict_value", json!({ "nested": "data" }));
    agent.state().set("null_value", json!(null));
    // --8<-- [end:state_validation]

    Ok(())
}

// --8<-- [start:state_in_tools]
/// Track user actions in agent state.
#[strands_agents::tool]
async fn track_user_action(
    /// The action to track.
    action: String,
    ctx: strands_agents::ToolContext,
) -> String {
    let state = ctx.agent.state();
    let count = state.get("action_count").and_then(|v| v.as_i64()).unwrap_or(0);

    state.set("action_count", count + 1);
    state.set("last_action", action.clone());

    format!("Action '{action}' recorded. Total actions: {}", count + 1)
}

/// Get user statistics from agent state.
#[strands_agents::tool]
async fn get_user_stats(ctx: strands_agents::ToolContext) -> String {
    let state = ctx.agent.state();
    let action_count = state.get("action_count").and_then(|v| v.as_i64()).unwrap_or(0);
    let last_action = state
        .get_as::<String>("last_action")
        .unwrap_or_else(|| "none".to_owned());

    format!("Actions performed: {action_count}, Last action: {last_action}")
}
// --8<-- [end:state_in_tools]

async fn state_in_tools_usage() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:state_in_tools_usage]
    use strands_agents::models::BedrockModel;
    use strands_agents::Agent;

    // Create agent with tools
    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(TrackUserActionTool::new())
        .tool(GetUserStatsTool::new())
        .build()?;

    // Use tools that modify and read state
    agent.invoke("Track that I logged in").await?;
    agent.invoke("Track that I viewed my profile").await?;
    println!("Actions taken: {:?}", agent.state().get("action_count"));
    println!("Last action: {:?}", agent.state().get("last_action"));
    // --8<-- [end:state_in_tools_usage]

    Ok(())
}

async fn invocation_state() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:invocation_state]
    use strands_agents::models::BedrockModel;
    use strands_agents::{Agent, InvocationState, InvokeOptions};

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // Pass per-invocation state when invoking
    let invocation_state = InvocationState::new();
    invocation_state.insert("request_id", "r-42".to_owned());
    invocation_state.insert("user_id", "u-1".to_owned());

    let result = agent
        .invoke_with(
            "Hi there!",
            InvokeOptions::default().with_invocation_state(invocation_state),
        )
        .await?;

    // Hooks and tools can read and mutate the invocation state during the
    // invocation. The same handle is returned on the result.
    println!("{:?}", result.invocation_state.get::<String>("request_id"));
    // Some("r-42")
    // --8<-- [end:invocation_state]

    Ok(())
}
