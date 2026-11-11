# Agent Development Guide - Strands Rust SDK

This guide covers `strands-rs/`, the Rust SDK for Strands Agents. Follow the root [AGENTS.md](../AGENTS.md) for monorepo-wide conventions; this file adds the Rust-specific rules.

## Purpose

The Rust SDK tracks the TypeScript SDK in `strands-ts/`. TypeScript is canonical for concepts, names, defaults, error text and wire formats. The Rust API uses the same names re-cased for Rust (`camelCase` to `snake_case`, `kebab-case` modules to `snake_case` with the same stem) and reads like idiomatic Rust: builders instead of option objects, `Result` instead of exceptions, `Stream` instead of async generators, cargo features instead of optional peer dependencies.

## Layout

```
Cargo.toml  Cargo.lock  deny.toml  rustfmt.toml  clippy.toml   # repo root: one virtual workspace
strands-rs/
├── strands/          # the SDK crate; providers, MCP and OpenTelemetry behind features
├── strands-macros/   # #[tool] and #[tools], re-exported by `strands`
├── conformance/      # TS-produced JSON fixtures and the tsx generator that writes them
└── docs/             # TESTING.md, PARITY.md, DIVERGENCES.md, reviews/
```

The workspace root is the repository root because Cargo needs one workspace above every member crate. The harness crate lives in `harness-rs/` (see [harness-rs/AGENTS.md](../harness-rs/AGENTS.md)).

Modules of `strands` mirror the stems of `strands-ts/src/` (`conversation-manager/` becomes `conversation_manager/`). Everyday names are re-exported explicitly from the crate root; TS-internal helpers stay `pub(crate)`.

### Cargo features

| Feature | Enables |
|---|---|
| `bedrock` (default) | `BedrockModel` on the AWS SDK |
| `anthropic` | `AnthropicModel` |
| `openai` | `OpenAIModel`; Bedrock Mantle also needs `bedrock` |
| `mcp` | `McpClient` on `rmcp` |
| `otel` | OpenTelemetry export |

A disabled feature drops its dependencies entirely. Gate every module and test file that needs a feature with `#[cfg(feature = "...")]` or `#![cfg(feature = "...")]`, so builds and tests pass with `--no-default-features`.

## Gates

Run these from the repository root before every commit. CI runs the same set.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo +1.96.0 check --workspace --all-features --locked   # MSRV
cargo deny check                                           # advisories, licenses, bans, sources
```

Also check each provider feature on its own (`--no-default-features --features anthropic`, and so on) when you touch feature gates.

## Code rules

- Edition 2024, MSRV 1.96. The MSRV never drops below what a required dependency declares and only moves in minor releases.
- Library code has no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!` or `unsafe`. Library code doesn't index or slice with `[]` either (clippy denies `indexing_slicing`; use `get`, iterators or pattern matching). Tests may use `unwrap`, `expect`, `panic!` and indexing: `clippy.toml` allows them inside `#[test]` functions and `#[cfg(test)]` modules, and each integration-test crate root (`tests/main.rs`) starts with `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::indexing_slicing, reason = "test code")]` so shared helpers under `tests/` are covered too.
- Every public item has rustdoc. Doc examples compile as doctests; use `no_run` when they need network or credentials, and `ignore` only when they genuinely can't compile.
- The public API never requires `Any`, `TypeId` or downcasting.
- `#[non_exhaustive]` policy: config and option structs (`AgentConfig`, `InvokeOptions`, `ModelRequest`, provider configs, `CancelOptions`, `InterruptParams`, tool config structs) are exhaustive, with public fields, `Default` and a builder, so `..Default::default()` works from other crates. Enums, events, errors, results and wire structs are `#[non_exhaustive]`, and wire structs get constructors (`new(..)` plus `with_*`). Never put `#[non_exhaustive]` on a config or option struct: it breaks `..Default::default()` construction from other crates (E0639).
- Where TS documents a default that differs from the type's zero value, implement `Default` by hand.
- No lock held across `.await` (clippy denies `await_holding_lock`, `await_holding_refcell_ref` and `await_holding_invalid_type`). Use `std::sync` locks for short critical sections and `tokio::sync` only where a lock must be held across an await. No process-global mutable state other than warn-once sets.
- No tokio types in public signatures. Every spawned future is instrumented (`.instrument(span)` or `.in_current_span()`).
- Errors are `thiserror` enums mirroring the TS error classes, with typed variants where callers branch and source chains. Interrupts are not errors.
- Counters use integer types and saturating arithmetic. Plain integer arithmetic is not linted (`arithmetic_side_effects` would flag every `+`), so review counters, lengths and offsets for overflow by hand and use `saturating_*` or `checked_*` where a value comes from input or accumulates.
- Prefer popular, maintained crates. If a dependency marks an API `#[deprecated]`, use a narrow `#[expect(deprecated, reason = "...")]`.
- Never set `panic = "abort"`: a panicking tool becomes an error tool result, which needs unwinding.
- Logging goes through `tracing` in the shared format `field=<value>, field=<value> | lowercase human-readable message`.
- Comments state only what a reader can't infer. Imports go at the top of the file. Keep control flow flat.
- Name constructs for what they do, never with a `Plugin` suffix.

### Naming and public surface

- TS names are re-cased, not renamed: types keep `PascalCase`, functions, fields and parameters become `snake_case`, and TS string literals become enum variants (`'endTurn'` becomes `StopReason::EndTurn`). Wire field names keep their wire format through serde renames.
- A name differs from TS only on a Rust prelude clash. Any other renamed or invented name is recorded in [docs/DIVERGENCES.md](docs/DIVERGENCES.md).
- Acronyms follow TS where TS has the type (`OpenAIModel`, `McpClient`); Rust-only types follow Rust conventions (`JsonValue`).
- The TS `experimental` entry point (`@strands-agents/sdk/experimental`) becomes the `strands::experimental` module.
- The TS `index.ts` barrel becomes explicit crate-root re-exports (no glob re-exports). TS `@internal` items are `pub(crate)`, or `#[doc(hidden)]` where another workspace crate needs them.
- Auxiliary model calls (summarization, memory extraction, classification, judging, web fetch) take an optional `model` and fall back to `agent.aux_model`, which defaults to the agent's model. Document the order as `this model > agent.aux_model > agent.model`.
- `invocation_state` belongs to the caller. SDK code never stores, keys or forwards internal state through it.
- No Rust-only behaviour: the SDK adds no features that TS lacks (small accessors such as `text_delta()` are fine). Where TS reaches into private agent internals, Rust exposes the narrowest public API that does the same job.

### Lints

The workspace lint table in the root `Cargo.toml` applies to every crate (`[lints] workspace = true`). It forbids `unsafe_code`, warns on `missing_docs` and the `rust_2018_idioms` group (CI turns warnings into errors), enables `clippy::all`, and adds:

- denied: `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable`, `indexing_slicing`, `dbg_macro`, `print_stdout`, `print_stderr`, `await_holding_lock`, `await_holding_refcell_ref`, `await_holding_invalid_type`;
- selected pedantic lints as warnings: `needless_pass_by_value`, `redundant_clone`, `large_futures`, the `cast_*` truncation and sign lints, `implicit_clone`, `inefficient_to_string`, `manual_let_else`, `match_same_arms`, `semicolon_if_nothing_returned`, `uninlined_format_args`, `unnested_or_patterns`, `redundant_closure_for_method_calls`, `missing_errors_doc`, `doc_markdown`.

The printer and examples are the only places that write to stdout; they opt out with a scoped `#[expect(clippy::print_stdout, reason = "...")]`. If a lint is wrong for one spot, use a narrowly scoped `#[expect(clippy::..., reason = "...")]`; never weaken the workspace table.

## Wire compatibility

Rust and TS read and write each other's data. These rules keep that true:

- Serde matches TS JSON: camelCase keys, `None` omitted, single-key block wrappers, text as a bare `{"text": ...}`, and key order (`serde_json` and `schemars` use `preserve_order`). Wire field names keep their wire format through serde renames (`inputSchema`, `toolUseId`).
- Closed TS unions that accept any string at runtime (media formats, citation locations, stop reasons, tool result status) deserialize unknown values into an `Other` fallback instead of failing. `Other` is never constructible with a known value.
- Base64 decoding is lenient like Node's `Buffer.from` (unpadded, URL-safe, whitespace); encoding uses the standard alphabet with padding.
- JS numbers that are integers stay integers: counters use `u64`, never `f64`.
- Where TS distinguishes `null` from a missing field (interrupt responses, `additionalModelResponseFields`, `trace`), a present `null` deserializes to `Some(JsonValue::Null)`.
- Snapshot keys are `<sessionId>/scopes/<scope>/<scopeId>/snapshots/{snapshot_latest.json, manifest.json, immutable_history/snapshot_<uuidv7>.json}`; ids match `^[a-z0-9_-]+$`. `Snapshot.created_at` keeps TS's millisecond `toISOString()` format. Rust writes compact JSON where TS pretty-prints; both parse either. `LocalFileStorage` writes atomically.
- Stash records use the TS class names for `storageType` (`LocalFileStorage`, `InMemoryStorage`).
- Span and metric names, units and `gen_ai.*` attributes match TS.
- Prompts and error text that cross-SDK tests check are byte-identical with TS. Everything else is Rust-native (slice strings by `char`, use `Duration`, use `Option` for unlimited).

Every user-visible behaviour difference from TS is recorded in [docs/DIVERGENCES.md](docs/DIVERGENCES.md). A divergence never changes a cross-SDK wire or persisted format.

## Testing

See [docs/TESTING.md](docs/TESTING.md). In short: the TS test suite is the behavioural spec; each Rust test is named after the TS `it(...)` title in snake_case; no real sleeps, no network in unit tests, no process env or cwd mutation; live tests are `#[ignore]`d and gated on credentials. [docs/PARITY.md](docs/PARITY.md) maps every in-scope TS spec file to its Rust tests.

## Conformance fixtures

`conformance/generate.ts` imports the built TS SDK and writes deterministic JSON fixtures to `conformance/fixtures/`. Rust tests parse each fixture, serialize it again and compare bytes. To regenerate:

```bash
npm ci && npm run build   # from the repository root
npm run generate --prefix strands-rs/conformance
git diff --exit-code strands-rs/conformance/fixtures
```

CI regenerates the fixtures and fails on any diff, so a TS wire change shows up as a fixture change in the same PR.
