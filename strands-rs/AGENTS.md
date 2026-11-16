# Agent development guide - Rust SDK

This guide is for agents and contributors working on the Strands Agents Rust SDK (`strands-rs/`). Cross-SDK rules (naming, parity, public and internal API marking, structured logging, evergreen comments) live in the [root AGENTS.md](../AGENTS.md).

## Purpose

`strands` is the Rust port of the Strands Agents SDK. The TypeScript SDK (`strands-ts/`) is canonical: concepts, names, defaults, error text and wire formats follow it, re-cased for Rust. The Rust API is still idiomatic Rust: builders instead of option literals, `Result` instead of exceptions, `#[tool]` instead of `tool({...})`, cargo features instead of optional peer dependencies, `Stream` instead of async generators, `shutdown().await` instead of `await using`.

## Layout

```
Cargo.toml                 # root virtual workspace (resolver 3, edition 2024, MSRV 1.96)
deny.toml rustfmt.toml clippy.toml
strands-rs/
  strands/                 # the SDK crate; modules mirror strands-ts/src file stems
  strands-macros/          # #[tool] and #[tools]
  conformance/             # TS-produced JSON fixtures and their generator
  docs/                    # TESTING.md, PARITY.md, DIVERGENCES.md, reviews/
harness-rs/                # the harness crate (strands-harness); see harness-rs/AGENTS.md
```

Cargo features of `strands`: `bedrock` (default), `anthropic`, `openai`, `mcp`, `otel`. Bedrock Mantle needs `openai` and `bedrock`. A disabled feature drops its dependencies; every module and test file that needs a feature is gated with `#[cfg(feature = "...")]`, so `--no-default-features` builds and tests pass.

## Gates

Run before every commit, from the repo root:

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

Never weaken a lint or skip a test to get green. Where a lint is wrong for one spot, use a narrowly scoped `#[expect(clippy::..., reason = "...")]`.

## Code rules

- Names: types keep the TS `PascalCase`; functions, fields and parameters are `snake_case`; enum-like literals are variants (`'endTurn'` is `StopReason::EndTurn`); modules use the TS stem in `snake_case` (`vended-plugins/` is `vended_plugins/`). A name changes only on a Rust prelude clash. Never name a construct with a `Plugin` suffix.
- Wire field names keep their wire format (`inputSchema`, `toolUseId`) through serde renames.
- Library code has no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!` or `unsafe`; the workspace lints enforce it. Tests may use `unwrap` and `expect`.
- The public API never requires `Any`, `TypeId` or downcasting. `ToolError` and `Error::Other` are the exception: they carry an open set of user errors, so `ToolError::downcast_ref::<E>()` and a `BoxError` downcast read a typed error back.
- `#[non_exhaustive]` policy: config and option structs are exhaustive, with public fields, `Default` and a builder, so `..Default::default()` works from other crates. Enums, events, errors, results and wire structs are `#[non_exhaustive]`, and wire structs get constructors (`new(..)` plus `with_*`). Where TS documents a default that differs from the zero value, `Default` is written by hand.
- No process-global mutable state other than warn-once sets. No lock held across `.await` (clippy denies `await_holding_lock`, `await_holding_refcell_ref` and `await_holding_invalid_type`). `std::sync` locks for short critical sections; `tokio::sync` only where a lock must be held across an await. Lazy init uses `OnceLock` or `tokio::sync::OnceCell`.
- No tokio types in public signatures. Every spawned future is instrumented, because `tracing` context does not follow `tokio::spawn`.
- Storage, memory stores and the local sandbox use `tokio::fs` or `spawn_blocking`, never blocking `std::fs` on a runtime thread.
- Counters (`Usage`, `Metrics`) are integers with saturating arithmetic.
- Rustdoc on every public item. Every doc example compiles as a doctest; `no_run` when it needs network or credentials.
- Comments state only what a reader can't infer. Imports at the top of the file. Flat control flow.
- Don't build with `panic = "abort"`: a panicking tool becomes an error tool result, which needs unwinding.
- TS `@internal` becomes `pub(crate)`, or `#[doc(hidden)]` where another crate in the workspace needs it. Crate-root re-exports are explicit; no glob re-exports.
- Structured logging goes through `tracing` in the `field=<value>, field=<value> | lowercase human-readable message` format.
- Acronyms follow TS where TS has the type (`OpenAIModel`, `McpClient`); Rust-only helper types follow Rust conventions (`JsonValue`).
- Outside byte-identical wire data, use Rust-native forms: `Duration`, not `f64` milliseconds; `Option<T>` for unlimited, not `Infinity`; slice strings by `char`, never by byte or UTF-16 code unit.
- No Rust-only features: v0.1 adds nothing TS lacks beyond small idiomatic accessors.
- Test code: integration-test helpers carry `#[cfg(test)]`, or the test file starts with `#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]`. Shared test helpers are `pub(crate)`.
- Examples use `main() -> Result` with `?`, never `unwrap` or `expect`.

### Public API rules

These hold across modules; a new type follows them without a new decision.

- Flat re-exports: a module re-exports its submodules' public names only where TS has an entry point for that folder (`vended_tools` and `vended_plugins` do; `vended_interventions` and `vended_memory_stores` don't, so their paths name the submodule). A flat set holds types, constants and functions, never builders.
- Public dependencies: a third-party crate whose types appear in a public signature is re-exported, from the module that uses it (`models::bedrock::aws_config`, `mcp::rmcp`) or from the crate root when several modules do (`strands::indexmap`).
- Builders: every public `bon` builder has `#[builder(state_mod(vis = "pub"))]`, and the builder and its state module are re-exported in every module-level `pub use` that names the config (`memory::{MemoryManagerConfig, MemoryManagerConfigBuilder, memory_manager_config_builder}`). The crate root does not re-export builders.
- `NAME`: a type with a fixed TS `name` has `pub const NAME: &'static str`, and its `name()` returns it. `ContextInjector::DEFAULT_NAME` is the one exception, because its config can replace the name.
- `Copy`: derived only on fieldless markers, C-like enums, newtypes over a `Copy` scalar (`HookOrder`) and borrowed context views. `#[non_exhaustive]` records and config and option structs derive `Clone` only, so a later non-`Copy` field breaks no caller.
- Shared handles: `SessionManager`, `MemoryManager`, `ContextManager`, `AgentSkills`, `ModelRouter`, `ContextInjector`, `StateStore`, `InvocationState`, `CancelHandle` and `CancelSignal` are cheap `Clone` handles to shared state. `HumanInTheLoop`, `CedarAuthorization`, `DefaultModelRetryStrategy`, the conversation managers and models are not `Clone`; share them with `Arc`. The single-value setters of `AgentBuilder` (`model`, `plugin`, `intervention`, `conversation_manager`, `retry_strategy`) accept a value, an `Arc<T>` or an `Arc<dyn Trait>`.
- Model providers: each has `XModel::new(options)` and `XModel::builder()`, whose setters are the options plus the flattened config fields.

### Selected lints

These match `[workspace.lints]` in the root `Cargo.toml`. Rust: `unsafe_code` is forbidden; `missing_docs`, `unreachable_pub` and the `rust_2018_idioms` group warn (errors under `-D warnings`). Clippy runs the `all` group as warnings, plus these lints from outside it, enabled one by one rather than whole groups:

- Denied: `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable`, `dbg_macro`, `print_stdout`, `print_stderr` (the printer and examples opt out explicitly), `await_holding_lock`, `await_holding_refcell_ref`, `await_holding_invalid_type`, `allow_attributes_without_reason`, `string_slice`, `disallowed_types`, `exhaustive_enums`, `exhaustive_structs`.
- Warned: `needless_pass_by_value`, `redundant_clone` (a nursery lint), `redundant_closure_for_method_calls`, `semicolon_if_nothing_returned`, `explicit_iter_loop`, `manual_let_else`, `match_same_arms`, `cast_possible_truncation`, `cast_sign_loss`, `cast_lossless`, `cast_possible_wrap`, `cast_precision_loss`, `inefficient_to_string`, `implicit_clone`, `uninlined_format_args`, `doc_markdown`, `missing_errors_doc`, `large_futures`, `indexing_slicing`.
- Counter modules (`Usage`, `Metrics` and wherever counters are summed) add `#![deny(clippy::arithmetic_side_effects)]` and use saturating ops.
- Config and option structs carry `#[expect(clippy::exhaustive_structs, reason = "config struct: struct-literal construction")]`; every other public struct and enum is `#[non_exhaustive]`.

`clippy.toml` denies `std::any::Any` and `std::any::TypeId` through `disallowed-types`, and allows `unwrap`, `expect`, `panic`, indexing, `dbg!` and printing in tests.

## Wire compatibility

Rust and TS read and write each other's data: messages, sessions, snapshots, interrupt state, the context stash, event JSON, metric and span names.

- JSON matches TS: camelCase keys, `None` omitted, single-key block wrappers, text blocks as a bare `{"text": ..}`, and TS key order (`serde_json` and `schemars` use `preserve_order`).
- Integers never gain `.0`; TS `number` counters are `u64`.
- Closed TS unions that accept any string at runtime (media formats, stop reasons, citation location types, tool result status) have an `Other` fallback, so unknown values never fail a whole message or snapshot.
- Base64 decoding is lenient like Node's `Buffer.from` (unpadded, URL-safe, whitespace); encoding uses the standard alphabet with padding.
- A present `null` where TS distinguishes `null` from absent (interrupt responses, `additionalModelResponseFields`, `trace`) round-trips as `Some(JsonValue::Null)`.
- Snapshot keys: `<sessionId>/scopes/<scope>/<scopeId>/snapshots/{snapshot_latest.json, manifest.json, immutable_history/snapshot_<uuidv7>.json}`. IDs match `^[a-z0-9_-]+$`. `createdAt` keeps the `toISOString()` millisecond format.
- `LocalFileStorage` writes atomically (temp file plus rename). Rust writes compact JSON where TS pretty-prints; both parse either.

## Testing

See [docs/TESTING.md](docs/TESTING.md). The TS test suite is the behavioural spec; [docs/PARITY.md](docs/PARITY.md) maps every TS spec file to its Rust tests, and [docs/DIVERGENCES.md](docs/DIVERGENCES.md) lists every user-visible difference.

## Conformance fixtures

`strands-rs/conformance/` holds JSON written by strands-ts and a deterministic generator. Regenerate from the repo root:

```bash
npm ci && npm run build
(cd strands-rs/conformance && npm run generate)
git diff --exit-code -- strands-rs/conformance/fixtures
```

Rust tests parse each fixture, serialize it again and compare the bytes.
