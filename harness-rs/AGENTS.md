# Agent development guide - Rust harness

This document provides guidance for AI agents working on the Strands harness for Rust (`harness-rs/`). For human contributors, see [CONTRIBUTING.md](../CONTRIBUTING.md).

> **The SDK rules apply here too.** Code rules, lints, wire compatibility, testing and live tests are stated once in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md); cross-SDK conventions (construct naming, parity of names, the evergreen-comment rule) live in the [root AGENTS.md](../AGENTS.md). This file covers only what is specific to the harness.

## Overview

`strands-harness` is a preconfigured Strands agent in one call, ported from `harness-ts/`, which is its specification. It is a thin composition layer over `strands`: `create_harness` wires a resolved model, the vended system prompt, built-in tools, context management, and optional session persistence into a plain `strands::Agent`. Every option is overridable, and the return value is a regular `Agent`.

The crate is library-only (the `strands` CLI stays in `strands-cli/`). It depends on `strands` with `mcp` enabled and forwards the provider features `bedrock` (default), `anthropic`, `openai` and `otel`.

## Directory structure

```
harness-rs/
├── AGENTS.md  CLAUDE.md  README.md
├── docs/
│   ├── PARITY.md              # one row per harness-ts spec file: ported/total and gaps
│   ├── DIVERGENCES.md         # every user-visible behaviour difference from harness-ts, with the reason
│   └── reviews/               # review findings and live-test results
└── strands-harness/
    ├── Cargo.toml
    └── src/
        ├── lib.rs             # crate docs and public re-exports
        ├── agent.rs           # create_harness(), HarnessAgentOptions
        ├── config.rs          # portable HarnessAgentConfig parsing into options
        ├── defaults.rs        # default model, effort, context manager, tools, session directory
        ├── models.rs          # "provider/name" model strings, per-provider effort and caching
        ├── prompt.rs          # HARNESS_CONTRACT and build_system_prompt()
        ├── interventions.rs   # interventions grammar
        ├── memory.rs          # memory option resolution
        ├── telemetry.rs       # explicit, idempotent telemetry setup
        ├── builtin_tools.rs   # built-in tool selection (list pin or mapping of edits)
        ├── sandbox.rs         # sandbox option resolution
        ├── tools/             # file_tools, shell, web_fetch, web_search, programmatic_tool_caller, subagent
        └── plugins/           # todos, environment
```

## Quality gates

The harness is a member of the root cargo workspace, so the gates in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md#quality-gates) cover it. To check the harness alone from the repo root:

```bash
cargo fmt --all --check
cargo clippy -p strands-harness --all-targets --all-features -- -D warnings
cargo clippy -p strands-harness --all-targets --no-default-features -- -D warnings
cargo test -p strands-harness --all-features
cargo test -p strands-harness --no-default-features
RUSTDOCFLAGS="-D warnings" cargo doc -p strands-harness --all-features --no-deps
cargo +1.96.0 check -p strands-harness --all-features --locked   # MSRV
cargo deny check
```

## Conventions

- **Follow harness-ts exactly** for defaults, the system prompt text, option precedence, model-string parsing, built-in tool names and descriptions, and error text. A bare model id means Bedrock; `bedrock/`, `bedrock-mantle/`, `anthropic/` and `openai/` select a provider.
- **Explicit wins.** `HarnessAgentOptions` carries every harness-ts field plus an `AgentConfig` passthrough. Any agent setting the caller passes wins over the harness default it corresponds to. Where the SDK has an object, the option accepts it directly and passes it through.
- **Accepted-but-ignored config is a bug.** Every config key either changes behaviour (with a test) or produces the harness-ts "unsupported" error. Module references in portable config are unsupported in Rust.
- **Built-in tools go through the sandbox.** File tools and environment discovery use `ctx.agent().sandbox()`, never host file APIs, so a custom sandbox sees every read, write and edit.
- **Subagents** are built from the parent's already-connected MCP clients passed as tools, never by re-passing `mcp_servers`, and are narrowed per server as in harness-ts. Delegates get recall-only memory and no extraction. Reuse the parent's discovery results where harness-ts does, forward child progress as full serialized child events, and resume child interrupts through `AgentResult::interrupts`. Presets keep harness-ts order.
- **No hidden global effects.** Telemetry setup is explicit and idempotent; constructors register nothing process-wide. Dates and platform come from `jiff` and `std::env::consts::OS`, never from shelling out.

## Error texts

- A provider harness-ts supports but Rust doesn't yet (`google`, `ollama`, `litellm`): `model provider "google" is not supported in Rust yet`.
- An unknown provider uses the harness-ts text: `Unknown model provider "x" in "x/y". Supported providers: .... Pass a Model instance for anything else.`
- A portable config with a module reference: `module references are not supported in Rust yet`, with ` (language "typescript")` appended when the reference names a language.
- A provider whose cargo feature is disabled: `model '<id>' requires the strands feature "<feature>"`.

## Byte-observable behaviour

These follow harness-ts byte for byte: system prompt assembly; tool-name collision rules, with `-` and `_` treated as equal; the `read`, `edit` and `todos` text; `EnvironmentContext` discovery limits; programmatic tool caller limits; `web_fetch` body caps and URL cache; session id sanitization.

## Persisted layout

Default sessions are written through `LocalFileStorage::new("./.agent").namespace("sessions")`, so the files match the harness-ts layout and a session written by either SDK resumes in the other. Snapshot keys, ID rules and timestamp formats follow [strands-rs/AGENTS.md](../strands-rs/AGENTS.md#persisted-layout).

## Testing

Follow [strands-rs/docs/TESTING.md](../strands-rs/docs/TESTING.md). The harness-ts specs under `harness-ts/test/` are the behavioural spec; track them in [docs/PARITY.md](docs/PARITY.md), with `harness-ts/test/integration/` as the live checklist.

- harness-ts tests `process.chdir` into temporary directories. Rust tests never change the current directory; the sandbox, the `./.agent` defaults and config loading take a crate-internal base-directory seam instead.
- Env-reading paths (API key fallbacks, `${VAR}` expansion, `OTEL_*`, `AWS_REGION`) take a crate-internal env-lookup seam. Never mutate process env in tests.
- Canonicalize paths in cwd assertions (macOS `/var` versus `/private/var`).
- Live tests follow the rules in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md#live-tests): credentials from the environment only, `live_*` names, assertions on the trace and side effects. Run them with `cargo test -p strands-harness --all-features -- --ignored live_` and record results in `docs/reviews/live.md`.
