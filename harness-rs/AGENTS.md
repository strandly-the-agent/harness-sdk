# Agent development guide - Rust harness

This guide is for agents and contributors working on the Strands Agents Rust harness (`harness-rs/`). Cross-SDK rules live in the [root AGENTS.md](../AGENTS.md); the Rust code rules, gates and wire-compatibility rules in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md) apply here unchanged.

## Purpose

`strands-harness` is the Rust port of the TypeScript harness (`harness-ts/`). `create_harness` builds an agent with the same defaults as harness-ts: built-in tools, sessions, context management, memory, interventions, skills and MCP servers. harness-ts is canonical for behaviour, prompt text, tool text and persisted layout.

## Layout

```
harness-rs/
  README.md AGENTS.md
  strands-harness/
    src/
      lib.rs            # crate docs and the flat re-exports; every module is private
      agent.rs          # create_harness, HarnessAgentOptions and the option enums
      types.rs          # Effort, Setting and the option config structs
      config.rs config/ # the portable JSON agent config: normalize, options
      error.rs          # HarnessError, Result
      defaults.rs models.rs models/ prompt.rs interventions.rs memory.rs telemetry.rs
      builtin_tools.rs  # which built-in tools an agent gets
      sandbox.rs        # the sandbox's shell probe
      env.rs logging.rs text.rs   # environment-lookup seam, warn_once, JS-compatible trim
      test_support.rs   # WarnRecorder (tests only)
      tools/            # file_tools, shell, web_fetch, web_search, programmatic_tool_caller, subagent
      plugins/          # todos, environment
    tests/              # outside-in tests: public_api.rs, programmatic_tool_caller_mcp.rs (Node
                        # fixture in fixtures/), live_e2e.rs with live_support/, cross_sdk/
  docs/                 # PARITY.md, DIVERGENCES.md, test-title-map.json, reviews/live.md
```

Cargo features forward to `strands`: `bedrock`, `anthropic`, `openai`, `mcp` and `otel`, all default (harness-ts always sets up telemetry).

## Gates

The workspace gates in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md#gates) cover this crate. To focus on it: `cargo test -p strands-harness --all-features` and `cargo clippy -p strands-harness --all-targets --all-features -- -D warnings`.

## Harness rules

- Byte-observable behaviour follows harness-ts: system prompt assembly, tool-name collision rules (`-` and `_` are equal), `read`, `edit` and `todos` text, `EnvironmentContext` discovery limits, programmatic tool caller limits, `web_fetch` caps and URL cache, session id sanitization.
- Default sessions are stored under `./.agent/sessions`, one folder per session id, the same paths harness-ts writes: the session manager's storage is `LocalFileStorage::new(dir).with_namespace("")` on the session directory, and the context manager's stash is in the session's own folder, `<dir>/<id>`.
- Every built-in tool goes through the agent's sandbox, never the host filesystem directly.
- Subagents are built from the parent's already-connected MCP clients, get recall-only memory, and follow harness-ts inheritance rules.
- Tests never change the process working directory or environment; the crate has internal seams for the base directory and environment lookup.

## Testing

`tests/public_api.rs` uses the crate as a dependent crate does: `create_harness` through the builder and the struct form, with `strands::testing` mock models and only crate-root names. Add a case there when a public option or conversion is added.

Port every applicable `it(...)` case from `harness-ts/test/`, named after its snake_case title; [docs/PARITY.md](docs/PARITY.md) tracks coverage and [docs/DIVERGENCES.md](docs/DIVERGENCES.md) lists differences. Live tests are `#[ignore = "live <provider>"]` and named `live_*`; see [strands-rs/docs/TESTING.md](../strands-rs/docs/TESTING.md#live-tests).

The title checker verifies every In scope row of [docs/PARITY.md](docs/PARITY.md) against the Rust test names, with the naming rules of [strands-rs/docs/TESTING.md](../strands-rs/docs/TESTING.md) and the declared mappings and gaps in [docs/test-title-map.json](docs/test-title-map.json). Run it from the repository root (it needs the `typescript` package: a checkout with `node_modules`, or `NODE_PATH`):

```bash
node strands-rs/docs/check-test-titles.mjs --parity harness-rs/docs/PARITY.md [--report] [filter...]
```

The `programmatic_tool_caller` MCP cases (`tests/programmatic_tool_caller_mcp.rs`) run a Node MCP server: set `STRANDS_TEST_NODE` to the node binary and `STRANDS_TEST_NODE_MODULES` to a `node_modules` with `@modelcontextprotocol/sdk` and `zod`; without them the cases skip with a message.
