# Agent development guide - Rust harness

This guide is for agents and contributors working on the Strands Agents Rust harness (`harness-rs/`). Cross-SDK rules live in the [root AGENTS.md](../AGENTS.md); the Rust code rules, gates and wire-compatibility rules in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md) apply here unchanged.

## Purpose

`strands-harness` is the Rust port of the TypeScript harness (`harness-ts/`). `create_harness` builds an agent with the same defaults as harness-ts: built-in tools, sessions, context management, memory, interventions, skills and MCP servers. harness-ts is canonical for behaviour, prompt text, tool text and persisted layout.

## Layout

```
harness-rs/
  strands-harness/
    src/
      lib.rs agent.rs config.rs defaults.rs models.rs prompt.rs interventions.rs memory.rs
      telemetry.rs builtin_tools.rs sandbox.rs
      tools/      # file_tools, shell, web_fetch, web_search, programmatic_tool_caller, subagent
      plugins/    # todos, environment
  docs/           # PARITY.md, DIVERGENCES.md, reviews/
```

Cargo features forward to `strands`: `bedrock`, `anthropic`, `openai`, `mcp` (all default) and `otel`.

## Gates

The workspace gates in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md#gates) cover this crate. To focus on it: `cargo test -p strands-harness --all-features` and `cargo clippy -p strands-harness --all-targets --all-features -- -D warnings`.

## Harness rules

- Byte-observable behaviour follows harness-ts: system prompt assembly, tool-name collision rules (`-` and `_` are equal), `read`, `edit` and `todos` text, `EnvironmentContext` discovery limits, programmatic tool caller limits, `web_fetch` caps and URL cache, session id sanitization.
- Default sessions use `LocalFileStorage::new("./.agent").namespace("sessions")`, the same paths harness-ts writes.
- Every built-in tool goes through the agent's sandbox, never the host filesystem directly.
- Subagents are built from the parent's already-connected MCP clients, get recall-only memory, and follow harness-ts inheritance rules.
- Tests never change the process working directory or environment; the crate has internal seams for the base directory and environment lookup.

## Testing

Port every applicable `it(...)` case from `harness-ts/test/`, named after its snake_case title; [docs/PARITY.md](docs/PARITY.md) tracks coverage and [docs/DIVERGENCES.md](docs/DIVERGENCES.md) lists differences. Live tests are `#[ignore = "live <provider>"]` and named `live_*`; see [strands-rs/docs/TESTING.md](../strands-rs/docs/TESTING.md#live-tests).
