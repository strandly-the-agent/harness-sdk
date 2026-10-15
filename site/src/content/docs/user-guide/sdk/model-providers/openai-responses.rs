//! Rust examples for the OpenAI Responses API documentation.

use strands_agents::Error;

async fn responses_basic_usage() -> Result<(), Error> {
    // --8<-- [start:basic_usage]
    use strands_agents::models::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let model = OpenAIModel::new(OpenAIModelConfig {
        model_id: Some("gpt-4o".to_owned()),
        api_key: Some("<KEY>".to_owned()),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    println!("{}", agent.invoke("Hello!").await?);
    // --8<-- [end:basic_usage]
    Ok(())
}

async fn web_search() -> Result<(), Error> {
    // --8<-- [start:web_search]
    use serde_json::{json, Map};
    use strands_agents::models::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let mut params = Map::new();
    params.insert("tools".to_owned(), json!([{ "type": "web_search" }]));

    let model = OpenAIModel::new(OpenAIModelConfig {
        model_id: Some("gpt-4o".to_owned()),
        api_key: Some("<KEY>".to_owned()),
        params,
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    let response = agent.invoke("What are the latest developments in AI?").await?;
    // --8<-- [end:web_search]
    let _ = response;
    Ok(())
}

async fn file_search() -> Result<(), Error> {
    // --8<-- [start:file_search]
    use serde_json::{json, Map};
    use strands_agents::models::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let mut params = Map::new();
    params.insert(
        "tools".to_owned(),
        json!([{ "type": "file_search", "vector_store_ids": ["vs_abc123"] }]),
    );

    let model = OpenAIModel::new(OpenAIModelConfig {
        model_id: Some("gpt-4o".to_owned()),
        api_key: Some("<KEY>".to_owned()),
        params,
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    let response = agent.invoke("What does the document say about pricing?").await?;
    // --8<-- [end:file_search]
    let _ = response;
    Ok(())
}

async fn code_interpreter() -> Result<(), Error> {
    // --8<-- [start:code_interpreter]
    use serde_json::{json, Map};
    use strands_agents::models::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let mut params = Map::new();
    params.insert(
        "tools".to_owned(),
        json!([{ "type": "code_interpreter", "container": { "type": "auto" } }]),
    );

    let model = OpenAIModel::new(OpenAIModelConfig {
        model_id: Some("gpt-4o".to_owned()),
        api_key: Some("<KEY>".to_owned()),
        params,
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    let response = agent.invoke("Calculate the SHA-256 hash of 'hello world'").await?;
    // --8<-- [end:code_interpreter]
    let _ = response;
    Ok(())
}

async fn remote_mcp() -> Result<(), Error> {
    // --8<-- [start:remote_mcp]
    use serde_json::{json, Map};
    use strands_agents::models::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let mut params = Map::new();
    params.insert(
        "tools".to_owned(),
        json!([{
            "type": "mcp",
            "server_label": "deepwiki",
            "server_url": "https://mcp.deepwiki.com/mcp",
            "require_approval": "never",
        }]),
    );

    let model = OpenAIModel::new(OpenAIModelConfig {
        model_id: Some("gpt-4o".to_owned()),
        api_key: Some("<KEY>".to_owned()),
        params,
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    let response = agent
        .invoke("Using deepwiki, what language is the harness-sdk repo written in?")
        .await?;
    // --8<-- [end:remote_mcp]
    let _ = response;
    Ok(())
}

async fn shell_tool() -> Result<(), Error> {
    // --8<-- [start:shell]
    use serde_json::{json, Map};
    use strands_agents::models::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let mut params = Map::new();
    params.insert(
        "tools".to_owned(),
        json!([{ "type": "shell", "environment": { "type": "container_auto" } }]),
    );

    let model = OpenAIModel::new(OpenAIModelConfig {
        api_key: Some("<KEY>".to_owned()),
        params,
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    let response = agent
        .invoke(r#"Use the shell to compute the md5sum of the string "hello world"."#)
        .await?;
    // --8<-- [end:shell]
    let _ = response;
    Ok(())
}

async fn stateful() -> Result<(), Error> {
    // --8<-- [start:stateful]
    use strands_agents::models::{OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let model = OpenAIModel::new(OpenAIModelConfig {
        model_id: Some("gpt-4o".to_owned()),
        api_key: Some("<KEY>".to_owned()),
        stateful: true,
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    agent.invoke("My name is Alice.").await?;
    // The agent's messages are empty: state is on the server

    let response = agent.invoke("What is my name?").await?;
    // The model remembers "Alice" via server-side state
    // --8<-- [end:stateful]
    let _ = response;
    Ok(())
}
