//! Rust examples for the OpenAI model provider documentation.
//! These examples demonstrate common usage patterns for the OpenAIModel.

use strands_agents::Error;

async fn basic_usage() -> Result<(), Error> {
    // --8<-- [start:basic_usage]
    use strands_agents::models::{OpenAIApi, OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let model = OpenAIModel::new(OpenAIModelConfig {
        api: OpenAIApi::Chat,
        api_key: Some("<KEY>".to_owned()),
        model_id: Some("gpt-5.4".to_owned()),
        max_tokens: Some(1000),
        temperature: Some(0.7),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    println!("{}", agent.invoke("What is 2+2").await?);
    // --8<-- [end:basic_usage]
    Ok(())
}

async fn custom_server() -> Result<(), Error> {
    // --8<-- [start:custom_server]
    use strands_agents::models::{OpenAIApi, OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    let model = OpenAIModel::new(OpenAIModelConfig {
        api: OpenAIApi::Chat,
        api_key: Some("<KEY>".to_owned()),
        base_url: Some("<URL>".to_owned()),
        model_id: Some("gpt-5.4".to_owned()),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;
    let response = agent.invoke("Hello!").await?;
    // --8<-- [end:custom_server]
    let _ = response;
    Ok(())
}

async fn gpt_oss_stop_sequences() -> Result<(), Error> {
    // --8<-- [start:gpt_oss_stop_sequences]
    use serde_json::json;
    use strands_agents::models::{OpenAIApi, OpenAIModel, OpenAIModelConfig};

    let mut params = serde_json::Map::new();
    params.insert("stop".to_owned(), json!(["<|call|>", "<|return|>", "<|end|>"]));

    let model = OpenAIModel::new(OpenAIModelConfig {
        api: OpenAIApi::Chat,
        api_key: Some("<KEY>".to_owned()),
        base_url: Some("http://localhost:8000/v1".to_owned()),
        model_id: Some("<MODEL_ID>".to_owned()),
        params,
        ..Default::default()
    })?;
    // --8<-- [end:gpt_oss_stop_sequences]
    let _ = model;
    Ok(())
}

async fn openai_structured_output() -> Result<(), Error> {
    // --8<-- [start:structured_output]
    use schemars::JsonSchema;
    use serde::Deserialize;
    use strands_agents::models::{OpenAIApi, OpenAIModel, OpenAIModelConfig};
    use strands_agents::Agent;

    /// Extract person information from text.
    #[derive(Debug, Deserialize, JsonSchema)]
    struct PersonInfo {
        /// Full name of the person.
        name: String,
        /// Age in years.
        age: u32,
        /// Job or profession.
        occupation: String,
    }

    let model = OpenAIModel::new(OpenAIModelConfig {
        api: OpenAIApi::Chat,
        api_key: Some("<KEY>".to_owned()),
        model_id: Some("gpt-4o".to_owned()),
        ..Default::default()
    })?;

    let agent = Agent::new(model)?;

    let person: PersonInfo = agent
        .structured_output("John Smith is a 30-year-old software engineer at a startup.")
        .await?;

    println!("Name: {}", person.name); // "John Smith"
    println!("Age: {}", person.age); // 30
    println!("Job: {}", person.occupation); // "software engineer"
    // --8<-- [end:structured_output]
    Ok(())
}
