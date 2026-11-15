# Agent development guide - Rust SDK

This document provides guidance for AI agents working on the Strands Rust SDK (`strands-rs/`). For human contributor guidelines, see [CONTRIBUTING.md](../CONTRIBUTING.md).

> **Cross-SDK rules live in the [root AGENTS.md](../AGENTS.md).** Plugin naming, cross-SDK parity, public and internal API marking, the structured-logging format, and the evergreen-comment rule apply to every SDK and are stated once there. This file shows only the Rust-idiomatic form and the rules unique to Rust.

## Overview

`strands` is the Rust port of the Strands Agents SDK. The TypeScript SDK (`strands-ts/`) is its specification: behaviour, text and wire formats follow TypeScript, and the Rust API follows Rust idiom where the two languages differ. Version 0.1 is a developer preview and adds no feature that TypeScript lacks.

Two crates make up the SDK:

- `strands`: the SDK. Cargo features `bedrock` (default), `anthropic`, `openai`, `mcp` and `otel`. Bedrock Mantle needs both `openai` and `bedrock`. A disabled feature drops its dependencies entirely.
- `strands-macros`: the `#[tool]` and `#[tools]` procedural macros, re-exported from `strands`.

The Rust harness (`harness-rs/`) builds on this crate; see [harness-rs/AGENTS.md](../harness-rs/AGENTS.md).

## Directory structure

```
Cargo.toml                 # cargo workspace root (repo root): members, [workspace.lints], [workspace.dependencies]
deny.toml  rustfmt.toml  clippy.toml
strands-rs/
├── AGENTS.md  CLAUDE.md  README.md
├── docs/
│   ├── TESTING.md         # how tests are written, named and run
│   ├── PARITY.md          # one row per TypeScript spec file: ported/total and gaps
│   ├── DIVERGENCES.md     # every user-visible behaviour difference from TypeScript, with the reason
│   └── reviews/           # review findings and live-test results
├── conformance/           # TypeScript-written JSON fixtures and their tsx generator
├── strands/               # the SDK crate: src/, tests/, examples/
└── strands-macros/        # #[tool] and #[tools]: src/{lib.rs, tool.rs, tools.rs}, tests/ (trybuild)
```

Modules of `strands` mirror the TypeScript `src/` stems in `snake_case`: `types`, `error`, `models` (the `Model` trait, streaming, model info, defaults, providers behind features, `routing`), `retry`, `hooks`, `tools` (tool, registry, function tool, executors, structured output), `agent` (agent, builder and config, stream, result, printer, agent as tool, invocation context, snapshot, tool caller), `state_store`, `interrupt`, `cancel`, `middleware`, `plugins`, `conversation_manager`, `interventions`, `vended_interventions` (`hitl`, `cedar`), `experimental` (`context_manager`), `injection`, `storage`, `session`, `memory`, `vended_memory_stores`, `sandbox`, `vended_tools`, `vended_plugins` (`context_injector`, `skills`), `background_tasks`, `mcp` (feature), `telemetry`, `logging` (warn-once) and `testing` (public test helpers).

The crate root re-exports the everyday names explicitly (no glob re-exports). TypeScript-internal helpers stay `pub(crate)`.

## Quality gates

Run these from the repo root before every commit. CI runs the same commands.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo test --workspace --all-features
cargo test --workspace --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc --workspace --all-features --no-deps
cargo +1.96.0 check --workspace --all-features --locked   # MSRV
cargo deny check                                         # advisories, licenses, bans, sources
```

Doctests run as part of `cargo test`. When you touch a feature-gated module, also build each provider feature on its own (for example `cargo check -p strands --no-default-features --features anthropic`). Redirect long build and test output to a file and read the tail.

Fix `cargo deny` advisories by choosing versions or features, not by ignoring them. If an ignore is unavoidable, scope it to one advisory with a reason in `deny.toml`.

## Code rules

### Naming

| Construct | TypeScript | Rust |
|---|---|---|
| Types | `PascalCase` | the same `PascalCase` |
| Functions, fields, parameters | `camelCase` | `snake_case` |
| Enum-like literals | `'endTurn'` | `StopReason::EndTurn` |
| Modules | `kebab-case` | `snake_case`, same stem (`vended-plugins/` to `vended_plugins/`) |
| `@strands-agents/sdk/experimental` | | `strands::experimental` |
| `index.ts` barrel, `@internal` | | crate-root re-exports; `pub(crate)` or `#[doc(hidden)]` |

- A name changes only when it clashes with the Rust prelude (the harness subagent `Option` becomes `ChoiceOption`).
- Name constructs for what they do, never with a `Plugin` suffix.
- Acronyms follow TypeScript where TypeScript has the type (`OpenAIModel`, `McpClient`); Rust-only helper types follow Rust conventions (`JsonValue`).
- Wire field names keep their wire format (`inputSchema`, `toolUseId`) through serde renames.
- Structured logs go through `tracing` in the form `field=<value>, field=<value> | lowercase human-readable message`.
- Auxiliary model calls (summarization, memory extraction, classification, judging, web fetch) take an optional `model` and fall back in the order `this model > agent.aux_model > agent.model`.
- `invocation_state` belongs to the user. SDK code never stores, keys or forwards internal state through it.

### Idiomatic Rust where Rust differs

Builders instead of optional-field object literals, `Result` instead of exceptions, `#[tool]` instead of `tool({...})`, cargo features instead of optional peer dependencies, `Stream` instead of async generators, and `shutdown().await` instead of `await using`.

### Byte identity

Wire formats, persisted data, prompts, and error text that cross-SDK tests check match TypeScript exactly. Everything else is Rust-native: slice strings by `char`, never by UTF-16 code unit; use `Duration`, not `f64` milliseconds; use `Option<T>` for "unlimited" instead of `Infinity`.

### No Rust-only behaviour

Don't add features TypeScript lacks. Small idiomatic accessors (like `text_delta()`) are fine. Where TypeScript reaches into private agent internals, expose the narrowest public API that does the same job (for example `ToolAgent::call_tool`). Record every user-visible difference from TypeScript in [docs/DIVERGENCES.md](docs/DIVERGENCES.md) with its reason. A fix never changes a cross-SDK wire or persisted format.

### Panic-free library code

Library code has no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!` or `unsafe`; the workspace lints enforce this. Tests may use `unwrap` and `expect`. Return a `strands::Error` instead. Never set `panic = "abort"` in a profile: a panicking tool is turned into an error result, which needs unwinding.

### Public API shape

- The public API never requires `Any`, `TypeId` or downcasting.
- **`#[non_exhaustive]` policy.** Config and option structs (`AgentConfig`, `InvokeOptions`, `ModelRequest`, provider configs, `CancelOptions`, `InterruptParams`, tool config structs) are exhaustive, with public fields, `Default` and a `bon` builder, so `..Default::default()` works from other crates. Never put `#[non_exhaustive]` on a config struct (E0639 breaks struct-literal construction). Enums, events, errors, results and wire structs are `#[non_exhaustive]`, and wire structs get constructors (`new(..)` plus `with_*`).
- Where TypeScript documents a default that differs from the type's zero value, implement `Default` by hand rather than deriving it.
- Prefer popular, well-maintained crates over hand-rolled code. New dependencies go in the root `[workspace.dependencies]` with only the features you use.

### Concurrency and locks

- No process-global mutable state other than warn-once sets, and no process-global side effects in constructors.
- Never hold a lock across `.await` (clippy `await_holding_*` lints deny it). Use `std::sync` locks for short critical sections and `tokio::sync` only where a guard must live across an await. Snapshot callbacks under a short lock and call them outside it.
- Use `std::sync::OnceLock` or `tokio::sync::OnceCell` for lazy initialization, never check-then-set.
- Library code never calls `block_in_place` or `Handle::block_on`. Use `tokio::fs` or `spawn_blocking` for file I/O on a runtime thread.
- Instrument every spawned future (`.instrument(span)` or `.in_current_span()`), because `tracing` context doesn't follow `tokio::spawn`.
- Child processes never outlive their tool: spawn them in their own process group with `kill_on_drop(true)` and kill the group on timeout, cancel and drop.
- No `tokio` types in public signatures; `CancelSignal` and `CancelHandle` wrap them.

### Rustdoc and doctests

Every public item has rustdoc (`missing_docs` is denied in CI). Every doc example compiles as a doctest: mark it `no_run` when it needs network or credentials, and never `ignore` unless it truly can't compile. Fallible public functions document their errors (`# Errors`). A hand-written `PartialEq` that ignores fields says so in its rustdoc.

### Code style

- `rustfmt` with `max_width = 110` and style edition 2024.
- Comment only what a reader can't infer: a constraint, an invariant, a non-obvious why. Never narrate a change ("previously", "now", "improved").
- Imports at the top of the file, never inside functions.
- Flat control flow: guard clauses, extracted helpers, lookup tables over branch ladders. Keep cognitive complexity and nesting low.
- Conventional commits: `feat(strands): ...`, `fix(strands): ...`, `test(strands): ...`, `docs: ...`, `ci: ...`.

## Lints

The lint set lives in `[workspace.lints]` in the root `Cargo.toml` and applies to every crate (`[lints] workspace = true`). Lints set to `warn` fail CI through `-D warnings`.

| Lints | Why |
|---|---|
| `unsafe_code = "forbid"` | The SDK needs no `unsafe`. This also rules out `std::env::set_var` in tests. |
| `missing_docs`, `unreachable_pub`, `rust_2018_idioms` | Every public item is documented, and `pub` means part of the public API. |
| clippy `all` | The default clippy set. |
| `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable` (deny) | Library code returns errors instead of panicking. `clippy.toml` allows `unwrap`, `expect` and `panic` in tests. |
| `dbg_macro`, `print_stdout`, `print_stderr` (deny) | Output goes through `tracing`. The stdout printer and examples opt out with a narrow `#[expect(..., reason = "...")]`. |
| `allow_attributes_without_reason` (deny) | Every lint exception says why; prefer `#[expect]` so a stale exception fails the build. |
| `await_holding_lock`, `await_holding_refcell_ref`, `await_holding_invalid_type` (deny) | Backs the no-lock-across-await rule. |
| `cast_lossless`, `cast_possible_truncation`, `cast_possible_wrap`, `cast_sign_loss` | Numeric conversions are explicit (`From`, `try_from`, saturating arithmetic), so counters can't silently wrap. |
| `needless_pass_by_value`, `redundant_clone`, `implicit_clone`, `cloned_instead_of_copied` | No needless allocation or ownership transfer in public signatures. |
| `missing_errors_doc`, `doc_markdown` | Fallible functions document their errors; identifiers in docs are code-formatted. |
| `manual_let_else`, `match_same_arms`, `explicit_iter_loop`, `redundant_closure_for_method_calls`, `semicolon_if_nothing_returned`, `uninlined_format_args`, `unnested_or_patterns`, `unused_async` | Selected pedantic lints for flat, readable code. |

If a dependency marks an API you must use as `#[deprecated]`, use a narrow `#[expect(deprecated, reason = "...")]`.

## Wire compatibility

Data written by one SDK must load in the others, so serialized shapes follow TypeScript, which the conformance fixtures check.

### Serde conventions

- JSON field names are `camelCase` through `#[serde(rename_all = "camelCase")]` or per-field renames. `None` fields are omitted on write.
- Content blocks use single-key wrappers (`{"toolUse": {...}}`), and text is a bare `{"text": "..."}`.
- Key order is part of the wire format and of prompt caching: `serde_json` and `schemars` use `preserve_order`. Never round-trip through a map without a stable order.
- Where TypeScript distinguishes `null` from a missing field (an interrupt response answered with `null`, `additionalModelResponseFields`, `trace`), a present `null` deserializes as `Some(JsonValue::Null)` through a custom deserializer with `#[serde(default)]`, and `None` is still skipped on write.
- `ToolChoice::Auto` and `ToolChoice::Any` serialize as `{"auto":{}}` and `{"any":{}}`, so they are empty struct variants, not unit variants.
- JSON config for tools uses typed structs with `rename_all = "camelCase"`, not raw maps.

### Wire types

- Closed TypeScript unions that accept any string at runtime (media formats, citation location types, tool result status) have an `Other(..)` fallback, so an unknown value (for example `heic`) never makes a message or snapshot fail to load.
- `StopReason::Other` can't hold a known value: its payload is a private newtype built through `From<&str>`, which maps known strings to their variants.
- Base64 decoding is lenient like Node's `Buffer.from` (unpadded, URL-safe, whitespace); encoding uses the standard alphabet with padding.
- Stream-event JSON from TypeScript writes byte arrays (for example `ReasoningContentDelta.redactedContent`) as index-keyed objects (`{"0":1,"1":2}`), while persisted blocks use base64. Both are accepted on read.
- Integers never gain a `.0`: TypeScript `number` counters (`Usage`, `Metrics`, citation positions) are integer types, and `f64` is kept only where values are genuinely fractional. Counters use saturating arithmetic.
- Text with lone UTF-16 surrogates and input beyond the `serde_json` recursion limit have defined, tested behaviour.
- `GuardContentBlock` is an enum (`Text | Image`) and fails to deserialize with the TypeScript messages (`GuardContentBlock must have either text or image content`, and the "cannot have both" text).
- Serde `Deserialize` is the saved-data path and keeps unknown blocks as `UnknownBlock` (with the TypeScript warning). Strict `from_data` constructors return `strands::Result` and fail with the TypeScript text (`Unknown ContentBlockData type`, `Unknown ToolResultContentData type`).
- Messages carry a `trackingId`; legacy data without one gains a minted id on load. `tracking_id` is private with a getter and `with_tracking_id`.
- Persisted interrupt-state structs are internal (`pub(crate)`), and public types don't expose `IndexMap`.
- `to_mime_type` and `to_media_format` follow `strands-ts/src/mime.ts`.

### Persisted layout

- Snapshot keys are `<sessionId>/scopes/<scope>/<scopeId>/snapshots/{snapshot_latest.json, manifest.json, immutable_history/snapshot_<uuidv7>.json}`.
- IDs match `^[a-z0-9_-]+$`.
- The stash `storageType` uses the TypeScript class names (`LocalFileStorage`, `InMemoryStorage`).
- `Snapshot.created_at` keeps the TypeScript `toISOString()` format with millisecond precision (`2026-11-11T12:00:00.000Z`).
- `LocalFileStorage` validates keys and writes atomically (temporary file plus rename). Rust writes compact JSON where TypeScript pretty-prints; both read either.

## Testing

Follow [docs/TESTING.md](docs/TESTING.md). In short:

- The TypeScript test suite is the behavioural spec. Port every applicable `it(...)` case, name the Rust test with the snake_case form of its title, and name the source spec file in the test module's doc comment. Update [docs/PARITY.md](docs/PARITY.md) in the same change.
- Unit tests live beside the code; cross-module behaviour goes in `strands/tests/`.
- Never sleep for real, never make network calls in unit tests, and never mutate process env or the current directory: use the injected sleeper, clock, env-lookup and base-directory seams.
- Assert wire shapes with `serde_json::json!` literals and exact TypeScript error text.
- Use the public `strands::testing` helpers (`MockModel`, `HookRecorder`, `test_agent(..)`).

### Live tests

Live tests are `#[ignore = "live <provider>"]`, named `live_*`, and skip with a message when credentials are missing. Run them explicitly:

```bash
cargo test -p strands --all-features -- --ignored live_
```

Credentials come from the environment only: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and the default AWS credential chain for Bedrock and Bedrock Mantle. Never print, log or commit a key, never dump the environment, and redact keys in every `Debug` impl. Live tests assert on the trace and side effects (tool name, parsed input, tool result, stop reason, usage), not on model wording. Record results in `docs/reviews/live.md`.

## Conformance fixtures

`conformance/` holds JSON written by the TypeScript SDK: messages and every content block, stream events, hook events, snapshots, session manifests and history, interrupt state, stash records, and metric and span names. Rust tests parse each fixture, serialize it back and compare against the TypeScript-written bytes; [conformance/README.md](conformance/README.md) describes the byte conventions and `conformance/fixtures/INDEX.md` lists every fixture. Regenerate from the repo root after a TypeScript wire change:

```bash
npm ci && npm run build
npx tsx strands-rs/conformance/generate.ts
```

The generator is deterministic (patched clocks, counter UUIDs and a seeded RNG) and rewrites `fixtures/` from scratch, so an unchanged TypeScript SDK produces identical files. CI regenerates the fixtures and fails on any diff, so commit regenerated fixtures together with the change that caused them. Never edit a fixture by hand.

## Quick rules

- Follow TypeScript for behaviour, text and wire formats; follow Rust idiom for API shape.
- No `unwrap`, `expect` or `panic!` in library code.
- Gate every feature-dependent module and test file with `#[cfg(feature = "...")]` so `--no-default-features` builds and tests pass.
- Don't change `strands-ts/` or any other SDK from a Rust change.
- Every user-visible difference from TypeScript goes in [docs/DIVERGENCES.md](docs/DIVERGENCES.md).
- Keep [docs/PARITY.md](docs/PARITY.md) current; a gap needs a reason.
