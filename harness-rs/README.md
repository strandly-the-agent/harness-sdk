# Strands harness for Rust

`strands-harness` is a preconfigured [Strands Agents](https://strandsagents.com) agent in one call. It wires a model, the harness system prompt, built-in tools (file tools, shell, web fetch, web search, a programmatic tool caller and subagents), context management and optional session persistence into a regular `strands::Agent`. Every option can be overridden.

It follows the [TypeScript harness](../harness-ts/) in options, defaults and persisted layout, and builds on the [Rust SDK](../strands-rs/).

## Status

Developer preview. The crate is not published to crates.io, and the API may change.

## Development

See [AGENTS.md](AGENTS.md) for conventions and [docs/PARITY.md](docs/PARITY.md) for test coverage against `harness-ts`. The crate builds from the repository root, which holds the Cargo workspace:

```bash
cargo test -p strands-harness --all-features
```

## License

Apache-2.0
