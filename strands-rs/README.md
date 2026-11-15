# Strands Agents Rust SDK

The Rust SDK for [Strands Agents](https://strandsagents.com), a model-driven framework for building AI agents. An agent is a model, a system prompt and a set of tools; the model decides which tools to call and when to stop.

This directory holds two crates:

- `strands` - agents, the agent loop, hooks, tools, model providers (Amazon Bedrock by default, Anthropic and OpenAI behind cargo features), MCP, sessions, memory and telemetry.
- `strands-macros` - the `#[tool]` and `#[tools]` attribute macros that turn Rust functions into agent tools.

The Rust SDK follows the [TypeScript SDK](../strands-ts/) in concepts, behaviour and wire formats, and is designed so that sessions and other persisted data are interchangeable between them.

## Status

Developer preview. The crates are not published to crates.io, and the API may change.

## Development

See [AGENTS.md](AGENTS.md) for the layout, gates and code rules, and [docs/TESTING.md](docs/TESTING.md) for how tests are written and run. The crates build from the repository root, which holds the Cargo workspace:

```bash
cargo test -p strands --all-features
```

## License

Apache-2.0
