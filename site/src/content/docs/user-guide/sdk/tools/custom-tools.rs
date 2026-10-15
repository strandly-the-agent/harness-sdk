//! Rust snippets for `custom-tools.mdx`.

fn basic_tool_scope() {
    // --8<-- [start:basic_tool]
    use strands_agents::tool;

    /// Get weather forecast for a city.
    #[tool]
    async fn weather_forecast(
        /// The name of the city.
        city: String,
        /// Number of days for the forecast.
        days: Option<u32>,
    ) -> String {
        let days = days.unwrap_or(3);
        format!("Weather forecast for {city} for the next {days} days...")
    }
    // --8<-- [end:basic_tool]
}

fn builder_tool_scope() {
    // --8<-- [start:builder_tool]
    use schemars::JsonSchema;
    use serde::Deserialize;
    use strands_agents::tools::{FunctionTool, ToolContext};

    #[derive(Deserialize, JsonSchema)]
    struct WeatherInput {
        /// The name of the city.
        city: String,
        /// Number of days for the forecast.
        days: Option<u32>,
    }

    let weather_tool = FunctionTool::builder("weather_forecast")
        .description("Get weather forecast for a city")
        .typed_handler(|input: WeatherInput, _ctx: ToolContext| async move {
            let days = input.days.unwrap_or(3);
            Ok(format!("Weather forecast for {} for the next {days} days...", input.city))
        });
    // --8<-- [end:builder_tool]
}

fn tool_override_scope() {
    // --8<-- [start:tool_override]
    use strands_agents::tool;

    /// Implementation function for weather forecasting.
    #[tool(
        name = "get_weather",
        description = "Retrieves weather forecast for a specified location"
    )]
    async fn weather_forecast(city: String, days: Option<u32>) -> String {
        let days = days.unwrap_or(3);
        format!("Weather forecast for {city} for the next {days} days...")
    }
    // --8<-- [end:tool_override]
}

fn json_schema_tool_scope() {
    // --8<-- [start:json_schema_tool]
    use serde_json::json;
    use strands_agents::tools::FunctionTool;

    let calculate_area = FunctionTool::builder("calculate_area")
        .description("Calculate area of a shape")
        .input_schema(json!({
            "type": "object",
            "properties": {
                "shape": {
                    "type": "string",
                    "enum": ["circle", "rectangle"],
                    "description": "The shape type"
                },
                "radius": { "type": "number", "description": "Radius for circle" },
                "width": { "type": "number", "description": "Width for rectangle" },
                "height": { "type": "number", "description": "Height for rectangle" }
            },
            "required": ["shape"]
        }))
        .handler(|input, _ctx| async move {
            let area = match input["shape"].as_str() {
                Some("circle") => {
                    let radius = input["radius"].as_f64().unwrap_or_default();
                    std::f64::consts::PI * radius * radius
                }
                Some("rectangle") => {
                    input["width"].as_f64().unwrap_or_default()
                        * input["height"].as_f64().unwrap_or_default()
                }
                _ => 0.0,
            };
            Ok(area)
        });
    // --8<-- [end:json_schema_tool]
}

async fn loading_tools_scope() -> Result<(), Box<dyn std::error::Error>> {
    use strands_agents::tool;

    /// Get weather forecast for a city.
    #[tool]
    async fn weather_forecast(city: String) -> String {
        format!("Weather forecast for {city}...")
    }

    // --8<-- [start:loading_tools]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::Agent;

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(WeatherForecastTool::new()).build()?;
    // --8<-- [end:loading_tools]
    Ok(())
}

fn tool_response_success_scope() {
    // --8<-- [start:tool_response_success]
    use serde::Serialize;
    use strands_agents::tool;

    #[derive(Serialize)]
    struct Forecast {
        city: String,
        days: u32,
        forecast: String,
    }

    /// Retrieves weather forecast for a specified location.
    #[tool(name = "get_weather")]
    async fn weather_forecast(city: String, days: Option<u32>) -> Forecast {
        let days = days.unwrap_or(3);
        Forecast {
            forecast: format!("Weather forecast for {city} for the next {days} days..."),
            city,
            days,
        }
    }
    // --8<-- [end:tool_response_success]
}

async fn async_tool_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:async_tool]
    use std::time::Duration;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::{tool, Agent};

    /// Call API asynchronously.
    #[tool]
    async fn call_api() -> String {
        tokio::time::sleep(Duration::from_secs(5)).await; // simulated api call
        "API result".to_owned()
    }

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(CallApiTool::new()).build()?;
    agent.invoke("Can you call my API?").await?;
    // --8<-- [end:async_tool]
    Ok(())
}

async fn tool_context_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:tool_context]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::ToolContext;
    use strands_agents::{tool, Agent};

    /// Get information about the agent.
    #[tool]
    async fn get_agent_info(ctx: ToolContext) -> String {
        format!("Agent has {} messages in history", ctx.agent.messages().len())
    }

    /// Get the tool use ID.
    #[tool]
    async fn get_tool_use_id(ctx: ToolContext) -> String {
        format!("Tool use is {}", ctx.tool_use.tool_use_id)
    }

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .tool(GetAgentInfoTool::new())
        .tool(GetToolUseIdTool::new())
        .build()?;

    agent.invoke("What is your information?").await?;
    agent.invoke("What is the tool use id?").await?;
    // --8<-- [end:tool_context]
    Ok(())
}

fn tool_context_name_scope() {
    // --8<-- [start:tool_context_name]
    use strands_agents::tool;
    use strands_agents::tools::ToolContext;

    /// Get information about the agent.
    #[tool]
    async fn get_self_name(context: ToolContext) -> String {
        format!("The agent name is {}", context.agent.name())
    }
    // --8<-- [end:tool_context_name]
}

async fn tool_context_invocation_state_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:tool_context_invocation_state]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::ToolContext;
    use strands_agents::{tool, Agent, InvocationState, InvokeOptions};

    /// Make an API call with user context.
    #[tool]
    async fn api_call(
        /// The search query.
        query: String,
        ctx: ToolContext,
    ) -> String {
        // Access per-invocation state via ctx.invocation_state
        let user_id = ctx.invocation_state.get::<String>("user_id");
        format!("searching {query} for {}", user_id.as_deref().map_or("", String::as_str))
    }

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(ApiCallTool::new()).build()?;

    // Pass invocation state when invoking
    let state = InvocationState::new();
    state.insert("user_id", "user123".to_owned());
    let options = InvokeOptions::default().with_invocation_state(state);
    let result = agent.invoke_with("Get my profile data", options).await?;
    // --8<-- [end:tool_context_invocation_state]
    Ok(())
}

async fn tool_context_app_state_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:tool_context_app_state]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::state_store::StateStore;
    use strands_agents::tools::ToolContext;
    use strands_agents::{tool, Agent};

    /// Look up the caller from durable agent state.
    #[tool]
    async fn get_profile(ctx: ToolContext) -> String {
        // Agent state is durable across invocations. `get_as` deserializes the
        // value instead of handing back a bare JsonValue.
        let state = ctx.agent.state();
        let user_id = state.get_as::<String>("user_id");
        let workspace_id = state.get_as::<String>("workspace_id");
        format!(
            "user={} workspace={}",
            user_id.unwrap_or_else(|| "unknown".to_owned()),
            workspace_id.unwrap_or_else(|| "none".to_owned())
        )
    }

    // Seed durable state once; every invocation's tools read it back.
    let state = StateStore::new();
    state.set("user_id", "user123");
    state.set("workspace_id", "ws456");

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent =
        Agent::builder().model(model).state(state).tool(GetProfileTool::new()).build()?;

    let result = agent.invoke("Show my profile").await?;
    // --8<-- [end:tool_context_app_state]
    Ok(())
}

async fn tool_streaming_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:tool_streaming]
    use futures::StreamExt;
    use std::time::{Duration, Instant};
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::ToolContext;
    use strands_agents::{tool, Agent, AgentStreamEvent};

    /// Process records with progress updates.
    #[tool]
    async fn process_dataset(
        /// Number of records to process.
        records: u32,
        ctx: ToolContext,
    ) -> String {
        let start = Instant::now();
        for i in 0..records {
            tokio::time::sleep(Duration::from_millis(100)).await;
            if i % 10 == 0 {
                let elapsed = start.elapsed().as_secs_f64();
                ctx.progress(format!("Processed {i}/{records} records in {elapsed:.1}s"))
                    .await;
            }
        }
        format!("Completed {records} records in {:.1}s", start.elapsed().as_secs_f64())
    }

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(ProcessDatasetTool::new()).build()?;

    let mut stream = agent.stream("Process 50 records");
    while let Some(event) = stream.next().await {
        if let AgentStreamEvent::ToolStreamUpdate(update) = event? {
            println!("Progress: {:?}", update.event.data);
        }
    }
    // --8<-- [end:tool_streaming]
    Ok(())
}

async fn class_multiple_tools_scope() -> Result<(), Box<dyn std::error::Error>> {
    // --8<-- [start:class_multiple_tools]
    use schemars::JsonSchema;
    use serde::Deserialize;
    use std::sync::Arc;
    use strands_agents::models::{BedrockModel, BedrockModelConfig};
    use strands_agents::tools::{FunctionTool, ToolContext};
    use strands_agents::Agent;

    #[derive(Deserialize, JsonSchema)]
    struct QueryInput {
        /// The SQL query to execute.
        sql: String,
    }

    struct Connection {
        db: String,
    }

    struct DatabaseTools {
        connection: Arc<Connection>,
    }

    impl DatabaseTools {
        fn new(connection_string: &str) -> Self {
            // Establish the connection once; every tool shares it.
            let db = format!("example_db ({connection_string})");
            Self { connection: Arc::new(Connection { db }) }
        }

        fn query_tool(&self) -> FunctionTool {
            let connection = Arc::clone(&self.connection);
            FunctionTool::builder("query_database")
                .description("Run a SQL query against the database")
                .typed_handler(move |input: QueryInput, _ctx: ToolContext| {
                    let connection = Arc::clone(&connection);
                    async move {
                        Ok(format!(
                            "Query results for {} on {}",
                            input.sql, connection.db
                        ))
                    }
                })
        }
    }

    let db_tools = DatabaseTools::new("example_connection_string");

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder().model(model).tool(db_tools.query_tool()).build()?;
    // --8<-- [end:class_multiple_tools]
    Ok(())
}

fn search_database_scope() {
    // --8<-- [start:search_database]
    use serde::Serialize;
    use strands_agents::tool;

    #[derive(Serialize)]
    struct Product {
        id: String,
        name: String,
        description: String,
        price: f64,
        category: Vec<String>,
    }

    #[tool(
        description = "Search the product database for items matching the query string.

Use this tool when you need to find detailed product information based on keywords,
product names, or categories. The search is case-insensitive and supports fuzzy
matching to handle typos and variations in search terms.

Example response:
[
  {
    \"id\": \"P12345\",
    \"name\": \"Ultra Comfort Running Shoes\",
    \"description\": \"Lightweight running shoes with...\",
    \"price\": 89.99,
    \"category\": [\"Footwear\", \"Athletic\", \"Running\"]
  }
]

Notes:
- This tool only searches the product catalog and does not provide inventory
  or availability information
- Results are cached for 15 minutes to improve performance
- The search index updates every 6 hours, so very recent products may not appear
- For real-time inventory status, use a separate inventory check tool"
    )]
    async fn search_database(
        /// The search string (product name, category, or keywords).
        /// Example: "red running shoes".
        query: String,
        /// Maximum number of results to return (default: 10, range: 1-100).
        max_results: Option<u32>,
    ) -> Vec<Product> {
        // Implementation would go here
        Vec::new()
    }
    // --8<-- [end:search_database]
}
