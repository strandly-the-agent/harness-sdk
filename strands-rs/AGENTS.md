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
- The public API never requires `Any`, `TypeId` or downcasting.
- `#[non_exhaustive]` policy: config and option structs are exhaustive, with public fields, `Default` and a builder, so `..Default::default()` works from other crates. Enums, events, errors, results and wire structs are `#[non_exhaustive]`, and wire structs get constructors (`new(..)` plus `with_*`). Where TS documents a default that differs from the zero value, `Default` is written by hand.
- No process-global mutable state other than warn-once sets. No lock held across `.await` (clippy denies `await_holding_lock`, `await_holding_refcell_ref` and `await_holding_invalid_type`). `std::sync` locks for short critical sections; `tokio::sync` only where a lock must be held across an await. Lazy init uses `OnceLock` or `tokio::sync::OnceCell`.
- No tokio types in public signatures. Every spawned future is instrumented, because `tracing` context does not follow `tokio::spawn`.
- Storage, memory stores and the local sandbox use `tokio::fs` or `spawn_blocking`, never blocking `std::fs` on a runtime thread.
- Counters (`Usage`, `Metrics`) are integers with saturating arithmetic.
- Rustdoc on every public item. Every doc example compiles as a doctest; `no_run` when it needs network or credentials.
- Comments state only what a reader can't infer. Imports at the top of the file. Flat control flow.
- Don't build with `panic = "abort"`: a panicking tool becomes an error tool result, which needs unwinding.

### Selected lints

`unsafe_code` is forbidden. `missing_docs`, `unreachable_pub` and the `rust_2018_idioms` group warn (errors under `-D warnings`). Clippy runs `all` plus these denies: `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `unreachable`, `dbg_macro`, `print_stdout`, `print_stderr` (the printer and examples opt out explicitly), `await_holding_lock`, `await_holding_refcell_ref`, `await_holding_invalid_type`, `allow_attributes_without_reason`; and a selective set of pedantic warnings (`needless_pass_by_value`, `redundant_clone`, `manual_let_else`, `match_same_arms`, `cast_*`, `uninlined_format_args`, `doc_markdown`, `missing_errors_doc`, and others listed in the root `Cargo.toml`). `clippy.toml` allows `unwrap`, `expect`, `panic`, `dbg!` and printing in tests.

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
