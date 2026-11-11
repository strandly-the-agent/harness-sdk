# Strands Agents SDK for Rust

`strands` is the Rust SDK for [Strands Agents](https://strandsagents.com), a model-driven approach to building AI agents. It is a developer preview that tracks the TypeScript SDK in `strands-ts/`: the same concepts, names and wire formats, re-cased for Rust. It tracks strands-ts and harness-ts at commit `7296caba7` on `main` (the spec commit); [docs/PARITY.md](docs/PARITY.md) and [docs/DIVERGENCES.md](docs/DIVERGENCES.md) refer to that commit.

The workspace has two crates here:

- `strands` (`strands/`): the SDK. Providers, MCP and OpenTelemetry are cargo features (`bedrock` is on by default; `anthropic`, `openai`, `mcp`, `otel` are opt-in).
- `strands-macros` (`strands-macros/`): the `#[tool]` and `#[tools]` macros, re-exported by `strands`.

The minimum supported Rust version is 1.96. See [AGENTS.md](AGENTS.md) for the development guide and [docs/TESTING.md](docs/TESTING.md) for how tests are organized.
