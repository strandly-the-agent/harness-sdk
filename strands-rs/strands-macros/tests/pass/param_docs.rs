//! Parameter doc comments become property descriptions and are stripped from the function.

use strands::{Tool, ToolError, tool};

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
    /// Temperature unit.
    unit: Option<String>,
) -> Result<String, ToolError> {
    Ok(format!("Sunny in {city} ({})", unit.unwrap_or_default()))
}

fn main() {
    assert_eq!(
        strands::types::json::to_string(&GetWeather.tool_spec()).unwrap(),
        concat!(
            r#"{"name":"get_weather","description":"Get the current weather for a city.","#,
            r#""inputSchema":{"type":"object","properties":{"#,
            r#""city":{"type":"string","description":"City name, for example \"Seattle\"."},"#,
            r#""unit":{"type":"string","description":"Temperature unit."}},"#,
            r#""required":["city"],"additionalProperties":false}}"#,
        )
    );
    let _ = get_weather;
}
