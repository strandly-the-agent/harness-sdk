# Strands Agents Rust SDK

`strands` is the Rust SDK for [Strands Agents](https://strandsagents.com), a model-driven approach to building AI agents. You give an agent a model, a system prompt and a set of tools, and the model decides what to do. The Rust SDK follows the TypeScript SDK (`strands-ts/`) for behaviour and data formats, with a Rust-native API.

## Status

This is a 0.x developer preview. The crates are not yet published to crates.io, and the API can change between previews. To try it, depend on a checkout of this repository by path.

## Features

| Area | What's included |
|---|---|
| Models | Amazon Bedrock (default), Bedrock Mantle, Anthropic and OpenAI providers; retry strategies; model routing with fallback and classifier strategies |
| Tools | `#[tool]` and `#[tools]` macros, concurrent and sequential executors, structured output, direct tool calls, agents as tools |
| Agent | `invoke` and streaming, invocation limits, cancellation, interrupts, app and model state, snapshots, background tasks |
| Extension | Hook events, plugins, middleware, interventions, human-in-the-loop and Cedar authorization |
| Context | Sliding-window and summarizing conversation managers, an experimental context manager, context injection, agent skills |
| Persistence and memory | Unified storage (in-memory and local files), snapshot-based sessions, memory manager and file memory store |
| Environment | A sandbox trait with a local sandbox, and vended tools such as `shell`, `read`, `write`, `edit`, `http_request` and `web_fetch` |
| MCP | `McpClient` over stdio, streamable HTTP and legacy SSE (`mcp` feature) |
| Telemetry | `tracing` spans and metrics with `gen_ai.*` attributes; OTLP export (`otel` feature) |

Cargo features: `bedrock` (default), `anthropic`, `openai`, `mcp`, `otel`. Bedrock Mantle needs both `openai` and `bedrock`.

## Example

A sketch of the intended API; see the crate documentation for examples that compile against the current preview.

```rust,ignore
use strands::{tool, Agent};

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
    let mut agent = Agent::builder()
        .system_prompt("You are a concise assistant.")
        .tools([GetWeather])
        .build()?;

    let result = agent.invoke("What's the weather in Seattle?").await?;
    println!("{result}");
    agent.shutdown().await
}
```

## Development

See [AGENTS.md](AGENTS.md) for the layout, code rules and quality gates, [docs/TESTING.md](docs/TESTING.md) for testing, [docs/PARITY.md](docs/PARITY.md) for test parity with the TypeScript SDK, and [docs/DIVERGENCES.md](docs/DIVERGENCES.md) for behaviour that differs from it.

## License

Apache-2.0. See [LICENSE.APACHE](../LICENSE.APACHE).
