# strands-macros

Procedural macros for the [Strands Agents](https://strandsagents.com) Rust SDK:

- `#[tool]` turns a function into a tool. The doc comment is the tool description and the parameters are the input schema.
- `#[tools]` turns the `#[tool]` methods of an impl block into tools that share the value's state.

Use them through the [`strands`](https://github.com/strands-agents/harness-sdk/tree/main/strands-rs/strands) crate, which re-exports both (`strands::tool`, `strands::tools`). Don't depend on this crate directly: the generated code names `strands` types.

```rust
use strands::tool;

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
) -> Result<String, strands::ToolError> {
    Ok(format!("Sunny in {city}"))
}
```

The attributes and what they generate are documented on the macros themselves. This crate follows the version of `strands` exactly.
