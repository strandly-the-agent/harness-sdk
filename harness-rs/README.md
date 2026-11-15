# Strands harness for Rust

`strands-harness` is a preconfigured [Strands Agents](https://strandsagents.com) agent in one call. `create_harness` wires a resolved model, the harness system prompt, built-in tools, context management and optional session persistence into a regular `strands::Agent`, and every option can be overridden. It follows the TypeScript harness (`harness-ts/`) for defaults, behaviour and on-disk formats.

## Status

This is a 0.x developer preview. The crate is not yet published to crates.io, and the API can change between previews. To try it, depend on a checkout of this repository by path.

## Features

| Area | What's included |
|---|---|
| Models | Model strings `bedrock/`, `bedrock-mantle/`, `anthropic/` and `openai/` (a bare id means Bedrock), with `effort` and `caching` |
| Built-in tools | `shell`, `read`, `write`, `edit`, `web_fetch`, `web_search` (native and Exa), `programmatic_tool_caller`, `subagent` with presets |
| Plugins | Todos and environment context |
| Configuration | `HarnessAgentOptions` with an `AgentConfig` passthrough, and portable `HarnessAgentConfig` parsing |
| Integrations | Interventions, MCP servers (a config file path or an inline map), sessions, skills and memory |

Cargo features: `bedrock` (default), `anthropic`, `openai`, `otel`.

## Example

A sketch of the intended API; see the crate documentation for examples that compile against the current preview.

```rust,ignore
use strands_harness::{create_harness, HarnessAgentOptions};

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut agent = create_harness(
        HarnessAgentOptions::builder()
            .model("global.anthropic.claude-opus-5")
            .mcp_servers("./mcp.json")
            .build(),
    )
    .await?;

    let result = agent.invoke("Fix the failing test in src/lib.rs").await?;
    println!("{result}");
    agent.shutdown().await
}
```

## Development

See [AGENTS.md](AGENTS.md) for the layout and conventions, [docs/PARITY.md](docs/PARITY.md) for test parity with the TypeScript harness, and [docs/DIVERGENCES.md](docs/DIVERGENCES.md) for behaviour that differs from it.

## License

Apache-2.0. See [LICENSE.APACHE](../LICENSE.APACHE).
