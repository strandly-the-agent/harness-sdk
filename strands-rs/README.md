# Strands Agents SDK for Rust

`strands` is the Rust SDK for [Strands Agents](https://strandsagents.com), a model-driven framework for building AI agents. It is a developer preview (v0.1) that tracks the TypeScript SDK in this repository for concepts, names and wire formats.

## Crates

- `strands`: the SDK. Model providers, MCP and OpenTelemetry are behind cargo features (`bedrock` by default; `anthropic`, `openai`, `mcp`, `otel`).
- `strands-macros`: `#[tool]` and `#[tools]`, re-exported from `strands`.

## Minimum supported Rust version

Rust 1.96.

## Development

See [AGENTS.md](AGENTS.md) for the layout, code rules and gates, and [docs/TESTING.md](docs/TESTING.md) for testing.
