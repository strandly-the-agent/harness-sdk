# Strands Agents SDK for Rust

`strands` is the Rust SDK for [Strands Agents](https://strandsagents.com), a model-driven framework for building AI agents. An agent is a model, a system prompt and a list of tools; the model's own reasoning decides which tools to call.

The crate is a developer preview (v0.1). It tracks the TypeScript SDK in this repository concept for concept, with Rust naming, and reads and writes the same JSON for messages, sessions and snapshots.

## Install

The crates are not published to a registry. Depend on a checkout of this repository:

```toml
[dependencies]
strands = { path = "../harness-sdk/strands-rs/strands" }
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
futures = "0.3"
```

or on the git repository (add `branch` or `rev` to pin what you build from):

```toml
strands = { git = "https://github.com/strands-agents/harness-sdk" }
```

A tool parameter or a structured output that is a type of your own derives `serde::Deserialize` and `schemars::JsonSchema`; add `serde = { version = "1", features = ["derive"] }` and `schemars = "1"` for those (the `schemars` major that `strands::schemars` re-exports).

The minimum supported Rust version is 1.96.

## Quick start

```rust
use futures::StreamExt;
use strands::{Agent, AgentStreamEvent, tool};

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
) -> Result<String, strands::ToolError> {
    Ok(format!("Sunny in {city}"))
}

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut agent = Agent::builder() // default model: Bedrock
        .system_prompt("You are a concise assistant.")
        .tools([GetWeather])
        .printer(false) // this example prints the output itself
        .build()?;

    let result = agent.invoke("What's the weather in Seattle?").await?;
    println!("{result}");

    {
        let mut stream = agent.stream("And in Lisbon?");
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::ModelStreamUpdate(update) = event?
                && let Some(text) = update.text_delta()
            {
                print!("{text}");
            }
        }
        println!();
    }
    agent.shutdown().await
}
```

This is [`examples/basic.rs`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/basic.rs). An agent built without a model uses Amazon Bedrock, with AWS credentials and the region from the default chain (environment, profile, instance role). To use another provider, enable its feature and pass a model:

```rust
use strands::models::AnthropicModel;

// The API key comes from `ANTHROPIC_API_KEY`.
let model = AnthropicModel::builder().model_id("claude-sonnet-4-6").build()?;
let mut agent = Agent::builder().model(model).build()?;
```

## Features

| Feature | Default | Enables |
|---|---|---|
| `bedrock` | yes | The Amazon Bedrock provider, which is also the default model |
| `anthropic` | no | The Anthropic provider |
| `openai` | no | The OpenAI provider (Responses and Chat Completions); with `bedrock`, also Amazon Bedrock Mantle |
| `mcp` | no | The Model Context Protocol client |
| `otel` | no | OpenTelemetry export |

```toml
strands = { path = "../harness-sdk/strands-rs/strands", default-features = false, features = ["anthropic", "mcp"] }
```

## Examples

Run an example from the repository root with `cargo run -p strands --example <name>`, adding `--features <feature>` where the table names one.

| Example | Shows | Needs |
|---|---|---|
| [`basic`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/basic.rs) | A tool, `invoke`, `stream` and `shutdown` | AWS credentials |
| [`tools`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/tools.rs) | `#[tool]`, `#[tools]`, `FunctionTool`, tool context, a direct tool call | AWS credentials |
| [`streaming`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/streaming.rs) | Handling stream events: text, tool calls, the final result | AWS credentials |
| [`hooks`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/hooks.rs) | Sync, ordered and async hooks, hook removal, cancellation | Nothing (scripted model) |
| [`structured_output`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/structured_output.rs) | A typed answer validated against a schema | AWS credentials |
| [`mcp`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/mcp.rs) | Tools from an MCP server over stdio | `--features mcp`, AWS credentials, `uvx` |
| [`sessions`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/sessions.rs) | Saving and restoring a conversation | Nothing (scripted model) |
| [`bedrock`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/bedrock.rs) | Configuring `BedrockModel` | AWS credentials |
| [`anthropic`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/anthropic.rs) | Configuring `AnthropicModel` | `--features anthropic`, `ANTHROPIC_API_KEY` |
| [`openai`](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/strands/examples/openai.rs) | Configuring `OpenAIModel` | `--features openai`, `OPENAI_API_KEY` |

## Crates

- [`strands`](https://github.com/strands-agents/harness-sdk/tree/main/strands-rs/strands): the SDK.
- [`strands-macros`](https://github.com/strands-agents/harness-sdk/tree/main/strands-rs/strands-macros): `#[tool]` and `#[tools]`, re-exported from `strands`.

## Documentation

- API reference: `cargo doc -p strands --all-features --open`. The crate page has a module overview and a table that maps TypeScript SDK names to Rust.
- [docs/PARITY.md](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/docs/PARITY.md): which TypeScript SDK tests have a Rust counterpart.
- [docs/DIVERGENCES.md](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/docs/DIVERGENCES.md): where the Rust SDK deliberately differs from the TypeScript SDK.
- [docs/TESTING.md](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/docs/TESTING.md): how the crate is tested.
- [AGENTS.md](https://github.com/strands-agents/harness-sdk/blob/main/strands-rs/AGENTS.md): layout, code rules and gates for contributors.
- [Strands Agents documentation](https://strandsagents.com): concepts shared by all the SDKs.
