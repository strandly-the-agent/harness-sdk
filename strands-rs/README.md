# Strands Agents SDK for Rust

A Rust port of the core of [`@strands-agents/sdk`](../strands-ts): a model, a system prompt, and
tools composed into an agent loop. **Experimental** — API may change; see
[docs/DESIGN.md](docs/DESIGN.md) for scope and [docs/PORT_REPORT.md](docs/PORT_REPORT.md) for
what is and isn't ported.

```rust
use std::sync::Arc;
use strands_agents::models::bedrock::BedrockModel;
use strands_agents::tools::FunctionTool;
use strands_agents::{Agent, Result};

#[tokio::main]
async fn main() -> Result<()> {
    let weather = FunctionTool::builder("weather")
        .description("Current weather for a city")
        .typed::<WeatherInput>()
        .typed_handler(|input: WeatherInput, _ctx| async move {
            Ok(serde_json::json!({ "city": input.city, "temp_c": 21 }))
        })
        .build()?;

    let agent = Agent::builder()
        .model(Arc::new(BedrockModel::default()))
        .system_prompt("You are a terse assistant.")
        .tool(Arc::new(weather))
        .build()
        .await?;

    let result = agent.invoke("What's the weather in Seattle?").await?;
    println!("{result}");
    Ok(())
}

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct WeatherInput {
    city: String,
}
```

## Features

- `bedrock` (default) — Amazon Bedrock Converse provider via `aws-sdk-bedrockruntime`.

## Development

```bash
cd strands-rs
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

See [AGENTS.md](AGENTS.md) for conventions and [docs/TESTING.md](docs/TESTING.md) for test rules.
