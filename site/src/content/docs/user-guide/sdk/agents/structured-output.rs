//! Snippets for `structured-output.mdx`.

// --8<-- [start:basic_usage]
use schemars::JsonSchema;
use serde::Deserialize;
use strands_agents::models::BedrockModel;
use strands_agents::Agent;

#[derive(Debug, Deserialize, JsonSchema)]
struct Person {
    /// Name of the person
    name: String,
    /// Age of the person
    age: u32,
    /// Occupation of the person
    occupation: String,
}

async fn basic_usage() -> Result<(), Box<dyn std::error::Error>> {
    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let person: Person =
        agent.structured_output("John Smith is a 30 year-old software engineer").await?;

    println!("{person:?}");
    Ok(())
}
// --8<-- [end:basic_usage]

async fn error_handling() -> Result<(), Box<dyn std::error::Error>> {
    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // --8<-- [start:error_handling]
    use strands_agents::Error;

    match agent.structured_output::<Person>("some prompt").await {
        Ok(person) => println!("{person:?}"),
        Err(Error::StructuredOutput(message)) => {
            println!("Structured output failed: {message}");
        }
        Err(error) => return Err(error.into()),
    }
    // --8<-- [end:error_handling]

    Ok(())
}

// --8<-- [start:auto_retries]
/// Schema constraints are validated on every attempt, and the errors go back to the
/// model so it can correct itself.
#[derive(Debug, Deserialize, JsonSchema)]
struct Name {
    #[schemars(extend("pattern" = "abc$"))]
    first_name: String,
}

async fn auto_retries() -> Result<(), Box<dyn std::error::Error>> {
    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let name: Name = agent.structured_output("What is Aaron's name?").await?;

    println!("{name:?}");
    Ok(())
}
// --8<-- [end:auto_retries]

// --8<-- [start:streaming]
#[derive(Debug, Deserialize, JsonSchema)]
struct WeatherForecast {
    location: String,
    temperature: i32,
    condition: String,
    humidity: u32,
    wind_speed: u32,
    forecast_date: String,
}

async fn streaming() -> Result<(), Box<dyn std::error::Error>> {
    use futures::StreamExt;
    use strands_agents::agent::AgentStreamEvent;
    use strands_agents::tools::input_schema_for;
    use strands_agents::InvokeOptions;

    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let options = InvokeOptions::default()
        .with_structured_output_schema(input_schema_for::<WeatherForecast>());
    let mut stream = agent.stream_with(
        "Generate a weather forecast for Seattle: 68°F, partly cloudy, 55% humidity, \
         8 mph winds, for tomorrow",
        options,
    );

    while let Some(event) = stream.next().await {
        match event? {
            AgentStreamEvent::ContentBlock(b) => println!("{:?}", b.content_block),
            AgentStreamEvent::AgentResult(event) => {
                let output = event.result.structured_output.unwrap_or_default();
                let forecast: WeatherForecast = serde_json::from_value(output)?;
                println!("The forecast for today is: {forecast:?}");
            }
            _ => {}
        }
    }

    Ok(())
}
// --8<-- [end:streaming]

// --8<-- [start:combining_tools]
#[derive(Debug, Deserialize, JsonSchema)]
struct NotesResult {
    /// The notebook that was updated
    notebook_name: String,
    /// The number of items added
    item_count: u32,
}

async fn combining_tools() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::tools::vended::FileEditorTool;

    let agent = Agent::builder()
        .model(BedrockModel::default_model().await?)
        .tool(FileEditorTool::default())
        .build()?;

    let notes: NotesResult = agent
        .structured_output(
            r#"Create a notebook named "ideas" and add three project ideas."#,
        )
        .await?;

    println!("{notes:?}");
    Ok(())
}
// --8<-- [end:combining_tools]

// --8<-- [start:multiple_types]
/// A person's basic information.
#[derive(Debug, Deserialize, JsonSchema)]
struct Contact {
    /// Full name
    name: String,
    /// Age in years
    #[schemars(range(min = 0, max = 150))]
    age: u32,
    /// Email address
    email: String,
    /// Phone number
    phone: Option<String>,
}

/// A task or todo item.
#[derive(Debug, Deserialize, JsonSchema)]
struct Task {
    /// Task title
    title: String,
    /// Detailed description
    description: String,
    /// Priority level: low, medium, high
    priority: String,
    /// Whether the task is completed
    completed: bool,
}

async fn multiple_types() -> Result<(), Box<dyn std::error::Error>> {
    let agent = Agent::new(BedrockModel::default_model().await?)?;

    let contact: Contact =
        agent.structured_output("Extract person: John Doe, 35, john@test.com").await?;
    let task: Task = agent
        .structured_output("Create task: Review code, high priority, completed")
        .await?;

    println!("{contact:?} {task:?}");
    Ok(())
}
// --8<-- [end:multiple_types]

// --8<-- [start:conversation_history]
#[derive(Debug, Deserialize, JsonSchema)]
struct CityInfo {
    city: String,
    country: String,
    population: Option<u64>,
    climate: String,
}

async fn conversation_history() -> Result<(), Box<dyn std::error::Error>> {
    let agent = Agent::new(BedrockModel::default_model().await?)?;

    // Build up conversation context
    agent.invoke("What do you know about Paris, France?").await?;
    agent.invoke("Tell me about the weather there in spring.").await?;

    // Extract structured information from the conversation
    let city: CityInfo = agent
        .structured_output(
            "Extract structured information about Paris from our conversation",
        )
        .await?;

    println!("City: {}", city.city); // "Paris"
    println!("Country: {}", city.country); // "France"
    Ok(())
}
// --8<-- [end:conversation_history]
