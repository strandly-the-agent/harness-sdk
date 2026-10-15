# Agent Development Guide - Rust Package

This document provides context and conventions for AI coding assistants working on the Strands
Agents Rust SDK (`strands-rs/`). For human contributors, see [CONTRIBUTING.md](../CONTRIBUTING.md).

> **Cross-package rules live in the [root AGENTS.md](../AGENTS.md).** This file shows only the
> Rust-idiomatic form and the rules unique to Rust.

## Overview

`strands-agents` is a port of the core of `strands-ts` (`@strands-agents/sdk`). The TypeScript
source and its tests are the behavioural spec; [docs/DESIGN.md](docs/DESIGN.md) records every
place the Rust shape deliberately diverges (handle-style `Agent`, serde enums for content blocks,
`Result`-returning tools, type-indexed hook registry). [docs/PORTING.md](docs/PORTING.md) is the
TS → Rust construct map; [docs/TESTING.md](docs/TESTING.md) the test conventions.

## Directory Structure

```
strands-rs/
├── Cargo.toml                    # workspace: strands-agents, strands-agents-macros, strands-harness, strands-cli-worker
├── crates/strands-agents-macros/ # `#[tool]` attribute macro (feature `macros`)
├── crates/strands-cli-worker/    # `strands-cli-worker` binary: the CLI's Rust runtime (docs/CLI_WORKER_PROTOCOL.md)
│   ├── src/{lib,runtime,protocol}.rs  # transport-agnostic worker loop, agent runtime, wire types
│   ├── src/main.rs               # stdin + fd 3 wiring, strands-harness agent factory
│   └── tests/worker.rs           # protocol round-trips over in-memory pipes with MockMessageModel
├── crates/strands-harness/       # the harness: create_harness(), defaults, prompt, models, builtin tools, plugins
│   ├── src/{agent,defaults,prompt,models,builtin_tools,memory,interventions}.rs
│   ├── src/tools/{file_tools,web_fetch,web_search,subagent}.rs
│   ├── src/plugins/{todos,environment}.rs
│   └── tests/{prompt_parity,integ_harness}.rs
├── crates/strands-agents/
│   ├── Cargo.toml                # features: bedrock (default)
│   ├── src/
│   │   ├── lib.rs                # crate root + top-level re-exports
│   │   ├── error.rs              # one `Error` enum, variant per TS error class
│   │   ├── types/                # Message, ContentBlock, media, ToolSpec, StopReason, SystemPrompt
│   │   ├── models/               # Model trait, stream events, stream_aggregated, bedrock, defaults
│   │   ├── tools/                # Tool trait, ToolContext, FunctionTool, registry, executors, vended/shell
│   │   ├── hooks/                # HookRegistry + every event struct
│   │   ├── plugins/              # Plugin trait, PluginRegistry, ModelPlugin, vended/{skills,context_injector}
│   │   ├── agent/                # Agent handle, builder, event loop, snapshot/checkpoint, as_tool, delegation, continuation
│   │   ├── storage/              # Storage trait, InMemoryStorage, LocalFileStorage, keyword search
│   │   ├── context_manager/      # ContextManager ('auto'/'agentic'), offload strategies, stash, retrieval tool
│   │   ├── background_tasks/     # background tool execution, manage tool, continuation delivery
│   │   ├── memory/               # MemoryManager, MemoryStore, ModelExtractor, FileMemoryStore, injection
│   │   ├── middleware/           # Stage trait, InvokeModelStage/ExecuteToolStage, MiddlewareRegistry
│   │   ├── interrupt.rs, interventions/  # interrupts, InterventionHandler, vended HumanInTheLoop
│   │   ├── sandbox/              # Sandbox trait, LocalSandbox
│   │   ├── telemetry/            # OpenTelemetry tracer/meter, AgentMetrics/AgentTrace
│   │   ├── mcp/                  # McpClient/McpTool over rmcp (feature mcp); src/bin/mcp_echo_server.rs for e2e
│   │   ├── models/{bedrock,anthropic,openai}/ + sse.rs
│   │   ├── session/              # SessionStorage, FileSessionStorage, SessionManager
│   │   ├── retry/                # backoff + model retry strategies
│   │   ├── state_store.rs        # JSON k/v store
│   │   └── logging.rs            # tracing helpers (warn_once)
│   ├── tests/                    # cross-module integration tests
│   └── examples/
└── docs/                         # DESIGN.md, PORTING.md, TESTING.md, PORT_REPORT.md
```

Module names mirror `strands-ts/src` in snake_case so a TS path maps mechanically to a Rust one.

## Commands

```bash
export PATH=$HOME/.cargo/bin:$PATH   # if rustup installed per-user
cd strands-rs
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo test --no-default-features      # must also build without Bedrock
cargo doc --no-deps
```

All four gates must be clean before a change is proposed. `missing_docs` is a warning (every
`pub` item needs a `///` line) and clippy runs with `-D warnings`.

## Conventions

- `src/bin/mcp_echo_server.rs` is force-tracked (`git add -f`): the root `.gitignore` excludes
  `bin/`. It is a test-only MCP server (`mcp-echo-server` feature) used by `tests/mcp_e2e.rs`.

- **Translate, don't improve.** A behaviour change relative to `strands-ts` is a bug unless
  `docs/DESIGN.md` says otherwise. Fix bugs upstream first, then port.
- **Public API shape.** `Agent` is a `Clone` handle (`&self` everywhere). Providers implement
  `Model` (object-safe; config on the concrete type). Tools implement `Tool` or use
  `FunctionTool::builder`. Hooks are typed structs; register with `agent.hooks().add_callback`.
- **Errors.** One `Error` enum; add a variant only when TS adds an error class. Tool failures are
  `Ok(ToolResultBlock { status: Error })`, never `Err`.
- **Wire compatibility.** JSON produced by serde must match TS `toJSON()` byte-for-byte in shape
  (camelCase, externally tagged content blocks, omitted `None`s). Session files written by one SDK
  must load in the other.
- **Async.** tokio runtime, `async_trait` for object-safe traits, `futures::stream::BoxStream` for
  streams, `tokio_util::sync::CancellationToken` for cancellation. No blocking I/O in async code.
- **No `unsafe`.** `#![forbid(unsafe_code)]` is set at the workspace level.
- **Dependencies.** Add to `[workspace.dependencies]` and reference with `.workspace = true`.
  Keep the default feature set small; provider SDKs go behind features.
- **Docs.** `///` on every public item, 1–4 lines, no narration. Comments only for non-obvious
  behaviour (a TS quirk being reproduced, an ordering constraint).

## Testing

See [docs/TESTING.md](docs/TESTING.md). Every TS `it(...)` has a Rust test with the same intent;
the traceability matrix lives in [docs/PORT_REPORT.md](docs/PORT_REPORT.md).

## Adding a model provider

- Implement `Model` for your type: `model_id()` plus `stream(&self, &[Message], &StreamOptions) ->
  ModelEventStream<'_>`; everything else (`stream_aggregated`, `aggregate`, `count_tokens`,
  `estimate_utilization`) has a default.
- Keep the config on the struct behind interior mutability (`RwLock<YourConfig>`) so `&self`
  methods can read it and `update_config` can replace it — see `models/bedrock`.
- Map provider errors onto `Error::ModelThrottled` (retryable), `Error::ContextWindowOverflow`
  (prompt too long) and `Error::model_with_source(msg, err)` for everything else; never leak the
  SDK's error type.
- Construct `StreamOptions`/`CountTokensOptions` with `..Default::default()` so new fields do not
  break you.
- Feature-gate the provider SDK dependency (`[features] your-provider = [...]`) and gate the
  module with `#[cfg(feature = "your-provider")]`.

## Known deviations from a literal port

A few APIs intentionally differ from `strands-ts` (fallible `build()`, `Agent` as a `Clone`
handle, `function_tool()` instead of a root `tool()`, prelude module). Each one is recorded with
its reasoning in the decision log in [docs/DESIGN.md](docs/DESIGN.md); check there before
"fixing" a difference you spot.
