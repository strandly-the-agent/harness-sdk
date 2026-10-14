# Strands Agents SDK for Rust

A Rust port of the core of [`@strands-agents/sdk`](../strands-ts): a model, a system prompt, and
tools composed into an agent loop. **Experimental** — API may change; see
[docs/DESIGN.md](docs/DESIGN.md) for scope, [docs/FEATURE_MATRIX.md](docs/FEATURE_MATRIX.md) for
feature availability across Python / TypeScript / Rust, and [docs/PORT_REPORT.md](docs/PORT_REPORT.md)
for what is and isn't ported.

```rust
use strands_agents::models::{BedrockModel, BedrockModelConfig};
use strands_agents::{tool, Agent};

/// Current weather for a city.
#[tool]
async fn weather(city: String) -> String {
    format!("It is 22°C and sunny in {city}.")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let model = BedrockModel::new(BedrockModelConfig::default()).await?;
    let agent = Agent::builder()
        .model(model)
        .system_prompt("You are a terse assistant.")
        .tool(WeatherTool::new())
        .build()
        .await?;

    let result = agent.invoke("What's the weather in Seattle?").await?;
    println!("{result}");
    Ok(())
}
```

`#[tool]` generates a `WeatherTool` struct (doc comment ⇒ description, parameters ⇒ JSON
schema) and comes from the default `macros` feature; `FunctionTool::builder` is the
runtime equivalent (see the `tools_builder` example).

More in `crates/strands-agents/examples/` (`basic`, `tools`, `tools_builder`, `hooks`,
`streaming`).

## Features

- `bedrock` (default) — Amazon Bedrock Converse provider via `aws-sdk-bedrockruntime`.
- `macros` (default) — the `#[tool]` attribute macro from `strands-agents-macros`.

## Development

```bash
cd strands-rs
cargo fmt --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test --all-features
```

See [AGENTS.md](AGENTS.md) for conventions and [docs/TESTING.md](docs/TESTING.md) for test rules.
