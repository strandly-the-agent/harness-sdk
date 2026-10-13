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
├── Cargo.toml                    # workspace (one crate today; harness-rs may join later)
├── crates/strands-agents/
│   ├── Cargo.toml                # features: bedrock (default)
│   ├── src/
│   │   ├── lib.rs                # crate root + top-level re-exports
│   │   ├── error.rs              # one `Error` enum, variant per TS error class
│   │   ├── types/                # Message, ContentBlock, media, ToolSpec, StopReason, SystemPrompt
│   │   ├── models/               # Model trait, stream events, stream_aggregated, bedrock, defaults
│   │   ├── tools/                # Tool trait, ToolContext, FunctionTool, registry, executors
│   │   ├── hooks/                # HookRegistry + every event struct
│   │   ├── plugins/              # Plugin trait, PluginRegistry, ModelPlugin
│   │   ├── agent/                # Agent handle, builder, event loop, AgentResult, InvocationState
│   │   ├── conversation_manager/ # Null + SlidingWindow
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
