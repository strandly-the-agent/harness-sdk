# Strands Agents harness for Rust

`strands-harness` builds a ready-to-run [Strands](https://strandsagents.com) agent in one call, with built-in tools, plugins, sessions and memory. It follows the TypeScript harness in `harness-ts/`, with the same defaults, and is built on the Rust SDK in `strands-rs/`. It tracks harness-ts and strands-ts at commit `7296caba7` on `main` (the spec commit); [docs/PARITY.md](docs/PARITY.md) and [docs/DIVERGENCES.md](docs/DIVERGENCES.md) refer to that commit.

The minimum supported Rust version is 1.96. See [AGENTS.md](AGENTS.md) for the development guide.
