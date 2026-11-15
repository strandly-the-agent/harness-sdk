# Agent development guide - Rust SDK

This document provides guidance for AI agents working on the Strands Rust SDK (`strands-rs/`). For human contributor guidelines, see [CONTRIBUTING.md](../CONTRIBUTING.md).

> **Cross-SDK rules live in the [root AGENTS.md](../AGENTS.md).** Plugin naming, cross-SDK parity, public and internal API marking, the structured-logging format and the evergreen-comment rule apply to every SDK and are stated once there. This file covers the Rust form of those rules and the rules unique to Rust.

## Overview

The Strands Rust SDK is the Rust port of the Strands Agents framework. It follows the TypeScript SDK (`strands-ts/`) in concepts, names, behaviour and wire formats, re-cased to Rust idiom. The TypeScript source and its tests are the behavioural spec: when the two disagree, the TypeScript behaviour wins unless [docs/DIVERGENCES.md](docs/DIVERGENCES.md) records the difference and its reason.

The SDK is a developer preview and is not published to crates.io.

## Directory structure

```
Cargo.toml  Cargo.lock             # root virtual workspace (shared with harness-rs/)
deny.toml  rustfmt.toml  clippy.toml
strands-rs/
├── AGENTS.md  CLAUDE.md  README.md
├── docs/
│   ├── TESTING.md                 # how tests are laid out, written and run
│   ├── PARITY.md                  # TS spec file -> Rust tests, ported/total per file
│   ├── DIVERGENCES.md             # user-visible differences from TS, with reasons
│   ├── LINTS.md                   # the workspace lint set
│   └── reviews/                   # review findings and their dispositions
├── conformance/                   # TS-produced JSON fixtures and their generator
├── strands/                       # the `strands` crate
│   ├── src/                       # modules mirror strands-ts/src/ stems
│   ├── tests/                     # cross-module tests
│   └── examples/
└── strands-macros/                # `#[tool]` and `#[tools]` proc macros
    ├── src/{lib.rs, tool.rs, tools.rs}
    └── tests/                     # trybuild compile tests
```

Modules of `strands` keep the TypeScript `src/` stems in snake case: `types`, `error`, `models` (providers under features, `routing`), `retry`, `hooks`, `tools`, `agent`, `state_store`, `interrupt`, `cancel`, `middleware`, `plugins`, `conversation_manager`, `interventions`, `vended_interventions`, `experimental`, `injection`, `storage`, `session`, `memory`, `vended_memory_stores`, `sandbox`, `vended_tools`, `vended_plugins`, `background_tasks`, `mcp`, `telemetry`, `logging` and `testing`. The crate root re-exports the everyday names explicitly; there are no glob re-exports.

Cargo features: `bedrock` (default), `anthropic`, `openai`, `mcp` and `otel`. Bedrock Mantle needs both `openai` and `bedrock`. A disabled feature drops its dependencies entirely, and every module and test file that needs a feature is gated with `#[cfg(feature = "...")]` so builds and tests pass with `--no-default-features`.

## Gates

Run these from the repository root before every commit. All of them must pass.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo +1.96.0 check --workspace --all-features --locked   # MSRV
cargo deny check                                          # advisories, licenses, bans, sources
```

Doctests run as part of `cargo test`. When you touch provider code, also check each provider feature on its own (`cargo check -p strands --no-default-features --features anthropic`, and so on). Fix `cargo deny` advisories by choosing features or versions; an ignore is scoped to one advisory and carries a reason.

The lint set is described in [docs/LINTS.md](docs/LINTS.md). When a lint exception is unavoidable, scope it to the smallest item and give a reason (`#[expect(lint, reason = "...")]`).

## Code rules

### Naming

| Construct | TypeScript | Rust |
|---|---|---|
| Types | `PascalCase` | the same `PascalCase` |
| Functions, fields, parameters | `camelCase` | `snake_case` |
| Enum-like literals | `'endTurn'` | `StopReason::EndTurn` |
| Modules | `kebab-case` | `snake_case`, same stem (`vended-plugins/` becomes `vended_plugins/`) |
| `experimental` entry point | `@strands-agents/sdk/experimental` | `strands::experimental` |
| `index.ts` barrels, `@internal` | | crate-root re-exports; `pub(crate)` or `#[doc(hidden)]` |

- Keep the TypeScript name. Change it only when it clashes with the Rust prelude (the harness subagent `Option` becomes `ChoiceOption`).
- Name constructs for what they do, never with a `Plugin` suffix.
- Acronyms follow TypeScript where TypeScript has the type (`OpenAIModel`, `McpClient`); Rust-only helper types follow Rust conventions (`JsonValue`).
- Wire field names keep their wire spelling (`inputSchema`, `toolUseId`) through serde renames.
- Structured logs go through `tracing` in the form `field=<value>, field=<value> | lowercase human-readable message`.
- Auxiliary model calls (summarization, memory extraction, classification, judging, web fetch) take an optional `model` and fall back in the order `this model > agent.aux_model > agent.model`.
- `invocation_state` is user-only. SDK code never stores, keys or forwards internal state through it.

### Idiomatic Rust where Rust differs

Use builders instead of optional-field object literals, `Result` instead of exceptions, `#[tool]` instead of `tool({...})`, cargo features instead of optional peer dependencies, `Stream` instead of async generators, and `shutdown().await` instead of `await using`.

### Byte identity

Wire formats, persisted data, prompts, and error text that tests or other SDKs check match TypeScript exactly. Everything else is Rust-native: slice strings by `char`, never by UTF-16 code unit; use `Duration`, not `f64` milliseconds; use `Option<T>` for "unlimited" instead of `Infinity`.

### No Rust-only behaviour

The Rust SDK adds no features that TypeScript lacks. Small idiomatic accessors (like `text_delta()`) are fine. Where TypeScript reaches into private agent internals, Rust exposes the narrowest public API that does the same job.

### Conservative code

- Library code has no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!` or `unsafe`. Tests may use `unwrap` and `expect`.
- The public API never requires `Any`, `TypeId` or downcasting.
- `#[non_exhaustive]` policy: config and option structs (`AgentConfig`, `InvokeOptions`, `ModelRequest`, provider configs, `CancelOptions`, `InterruptParams`, tool config structs) are exhaustive, with public fields, `Default` and a `bon` builder, so `..Default::default()` works from other crates. Enums, events, errors, results and wire structs are `#[non_exhaustive]`, and wire structs get constructors (`new(..)` plus `with_*`). Never put `#[non_exhaustive]` on a config struct.
- When TypeScript documents a default that differs from the type's zero value, implement `Default` by hand instead of deriving it.
- Prefer popular, well-maintained crates over hand-rolled code.
- No process-global mutable state other than warn-once sets. Never hold a lock across `.await`. Use `std::sync` locks for short critical sections and `tokio::sync` only where a lock must be held across an await. Use `OnceLock` or `tokio::sync::OnceCell` for lazy initialisation, never check-then-set.
- Never set `panic = "abort"`: a panicking tool becomes an error tool result, which needs unwinding.
- Every public item has rustdoc. Every doc example compiles as a doctest; mark it `no_run` when it needs network or credentials, and use `ignore` only when it truly can't compile.

### Code style

- Comment only what a new reader can't infer: a constraint, an invariant, a non-obvious why. Never narrate a change ("previously", "now", "improved").
- Imports go at the top of the file, never inside functions.
- Keep control flow flat: guard clauses, extracted helpers, lookup tables instead of branch ladders.
- Use conventional commits.

## Wire compatibility

Data written by one SDK must load in the others. These rules keep the formats aligned; the [conformance fixtures](#conformance-fixtures) check them.

- `serde_json` and `schemars` use `preserve_order`: key order is part of the wire format and of prompt caching.
- Closed TypeScript unions that accept any string at runtime (media formats, citation location types, tool result status, stop reasons) get an `Other(..)` fallback, so an unknown value never fails a whole message or snapshot. `StopReason::Other` can't hold a known value: it is built through `From<&str>`, which maps known strings to their variants.
- Base64 decoding is lenient like Node's `Buffer.from` (unpadded, URL-safe, whitespace); encoding uses the standard alphabet with padding. Byte fields that TypeScript stream events serialize as index-keyed objects (`{"0":1,"1":2}`) are accepted in that form as well as base64.
- TypeScript `number` counters (`Usage`, `Metrics`, citation positions) are integer types, so they never gain a `.0`. Counters use saturating arithmetic.
- Where TypeScript distinguishes `null` from a missing field (interrupt responses, `additionalModelResponseFields`, `trace`), a present `null` deserializes to `Some(JsonValue::Null)` and `None` is skipped on write.
- `serde::Deserialize` is the lenient saved-data path (unknown content blocks are kept, with the TypeScript warning). The strict `from_data` constructors return the TypeScript errors (`Unknown ContentBlockData type`, `Unknown ToolResultContentData type`).
- Messages carry a `trackingId`; legacy data without one gains a minted id on load.
- Persisted interrupt-state structs are internal (`pub(crate)`).

Persisted layout:

- Snapshot keys are `<sessionId>/scopes/<scope>/<scopeId>/snapshots/{snapshot_latest.json, manifest.json, immutable_history/snapshot_<uuidv7>.json}`, and ids match `^[a-z0-9_-]+$`.
- The stash `storageType` uses the TypeScript class names (`LocalFileStorage`, `InMemoryStorage`).
- `Snapshot.created_at` keeps the millisecond `toISOString()` format (`2026-11-11T12:00:00.000Z`).
- `LocalFileStorage` writes atomically (temporary file plus rename) and validates keys. Rust writes compact JSON where TypeScript pretty-prints; both read either.

A change must never alter a cross-SDK wire or persisted format. If a TypeScript behaviour is wrong, fix it in Rust only where the formats stay interoperable, and record the difference in [docs/DIVERGENCES.md](docs/DIVERGENCES.md).

## Testing

Follow [docs/TESTING.md](docs/TESTING.md). In short:

- Port every applicable TypeScript `it(...)` case from each in-scope spec file. Name each Rust test with the snake_case form of the TypeScript title, and name the source spec file in the test module's doc comment (`//! Ported from strands-ts/src/agent/__tests__/agent.cancel.test.ts`).
- Record progress in [docs/PARITY.md](docs/PARITY.md). A spec case that isn't ported needs a reason (out of scope, TypeScript-only mechanism, deprecated).
- Unit tests live beside the code; cross-module behaviour goes in `strands/tests/`.
- Never sleep for real, never make network calls in unit tests, never change the process environment or working directory in tests.
- Assert wire shapes with `serde_json::json!` literals and exact TypeScript error text.
- Live tests are `#[ignore = "live <provider>"]`, named `live_*`, and skip with a message when credentials are missing.

## Conformance fixtures

`strands-rs/conformance/fixtures/` holds JSON produced by the TypeScript SDK (messages and content blocks, stream events, hook events, snapshots, session manifests, interrupt state, stash records, metric and span names). Rust tests deserialize, re-serialize and compare against them. Regenerate them from the repository root:

```bash
npm ci && npm run build
npx tsx strands-rs/conformance/generate.ts
```

CI regenerates the fixtures and fails if the result differs from what is committed. Never edit a fixture by hand.

## Development commands

```bash
cargo test -p strands --all-features                   # SDK tests
cargo test -p strands --all-features sliding_window    # filter by name
cargo test -p strands --no-default-features            # without Bedrock
cargo test -p strands-macros                           # macro and trybuild tests
cargo test -p strands --all-features -- --ignored live_   # live tests (needs credentials)
cargo doc -p strands --all-features --no-deps --open
```
