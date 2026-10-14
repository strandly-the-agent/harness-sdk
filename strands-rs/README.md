# Strands Agents SDK for Rust

A Rust port of the core of [`@strands-agents/sdk`](../strands-ts): a model, a system prompt, and
tools composed into an agent loop. **Experimental** — API may change; see
[docs/DESIGN.md](docs/DESIGN.md) for scope and [docs/PORT_REPORT.md](docs/PORT_REPORT.md) for
what is and isn't ported.

```rust
use schemars::JsonSchema;
use serde::Deserialize;
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::tools::{FunctionTool, ToolContext};
use strands_agents::Agent;

#[derive(Deserialize, JsonSchema)]
struct WeatherInput {
    city: String,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let weather = FunctionTool::builder("weather")
        .description("Current weather for a city")
        .typed_handler(|input: WeatherInput, _ctx: ToolContext| async move {
            Ok(format!("It is 22°C and sunny in {}.", input.city))
        });

    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("You are a terse assistant.")
        .tool(weather)
        .build()
        .await?;

    let result = agent.invoke("What's the weather in Seattle?").await?;
    println!("{result}");
    Ok(())
}
```

More in `crates/strands-agents/examples/` (`basic`, `tools`, `hooks`, `streaming`).

## Features

- `bedrock` (default) — Amazon Bedrock Converse provider via `aws-sdk-bedrockruntime`.

## Development

```bash
cd strands-rs
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

See [AGENTS.md](AGENTS.md) for conventions and [docs/TESTING.md](docs/TESTING.md) for test rules.
