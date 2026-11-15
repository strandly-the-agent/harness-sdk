# Agent development guide - Rust harness

This document provides guidance for AI agents working on the Strands harness Rust crate (`harness-rs/`). For human contributor guidelines, see [CONTRIBUTING.md](../CONTRIBUTING.md).

> **Shared rules live elsewhere.** Cross-package rules (public-surface parity with `harness-ts/` and `harness-py/`, the shared defaults, the single system-prompt artifact, the evergreen-comment rule) are in the [root AGENTS.md](../AGENTS.md). The Rust code rules, gates, wire-compatibility rules and testing rules are in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md) and apply here unchanged. This file covers only what is specific to the harness.

## Overview

`strands-harness` is a preconfigured Strands agent in one call. It is a thin composition layer over the `strands` crate: it wires a resolved model, the vended system prompt, built-in tools, context management and optional session persistence into a plain `strands::Agent`. Every option is overridable, and the result is a regular `Agent`.

The harness follows `harness-ts/` in options, defaults, prompts, tool text and persisted layout. The TypeScript source and tests in `harness-ts/` are the behavioural spec; differences are recorded in [docs/DIVERGENCES.md](docs/DIVERGENCES.md).

The crate is a developer preview, is library-only (no CLI) and is not published to crates.io.

## Directory structure

```
harness-rs/
├── AGENTS.md  CLAUDE.md  README.md
├── docs/
│   ├── PARITY.md                  # harness-ts spec file -> Rust tests, ported/total per file
│   ├── DIVERGENCES.md             # user-visible differences from harness-ts, with reasons
│   └── reviews/                   # review findings and their dispositions
└── strands-harness/
    └── src/
        ├── lib.rs
        ├── agent.rs               # create_harness(): the one-call factory
        ├── config.rs              # portable HarnessAgentConfig parsing
        ├── defaults.rs            # default model, effort, context manager, tools, session dir
        ├── models.rs              # "provider/name" -> model, per-provider effort config
        ├── prompt.rs              # HARNESS_CONTRACT and build_system_prompt()
        ├── interventions.rs  memory.rs  telemetry.rs  builtin_tools.rs  sandbox.rs
        ├── tools/                 # file_tools, shell, web_fetch, web_search,
        │                          # programmatic_tool_caller, subagent
        └── plugins/               # todos, environment
```

Cargo features `bedrock` (default), `anthropic` and `openai` forward to the matching `strands` features.

## Conventions

- **Options, explicit wins.** `HarnessAgentOptions` carries every `harness-ts` option plus an `AgentConfig` passthrough. A value the caller sets wins over the harness default it corresponds to.
- **Model strings** are `bedrock/<id>`, `bedrock-mantle/<id>`, `anthropic/<id>` and `openai/<id>`; a bare id means Bedrock. Errors users see:
  - a provider whose cargo feature is disabled: `model 'anthropic/...' requires the strands feature "anthropic"`;
  - a provider `harness-ts` supports but this crate doesn't: `model provider "google" is not supported in Rust yet`;
  - an unknown provider: the `harness-ts` text, `Unknown model provider "x" in "x/y". Supported providers: ...`;
  - a module reference in a portable config: `module references are not supported in Rust yet`, with ` (language "typescript")` appended when the reference names a language.
- **Byte-observable behaviour follows `harness-ts`:** system prompt assembly, tool-name collision rules (with `-` and `_` treated as equal), the `read`, `edit` and `todos` text, `EnvironmentContext` discovery limits, programmatic tool caller limits, `web_fetch` caps and URL cache, and session id sanitization.
- **Default sessions** are written through `LocalFileStorage::new("./.agent").namespace("sessions")`, so the files land where `harness-ts` puts them and either SDK can resume a session the other wrote.
- **`monty`** (the programmatic tool caller's interpreter) is a dependency of this crate only, never of `strands`.

## Gates

The workspace gates in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md#gates) cover this crate. To work on the harness alone:

```bash
cargo clippy -p strands-harness --all-targets --all-features -- -D warnings
cargo test -p strands-harness --all-features
cargo test -p strands-harness --no-default-features
```

## Testing

Follow [strands-rs/docs/TESTING.md](../strands-rs/docs/TESTING.md) and record progress in [docs/PARITY.md](docs/PARITY.md). Rules that matter most here:

- `harness-ts` tests `process.chdir` into temporary directories. Rust tests never change the working directory; the local sandbox, the `./.agent` defaults and config loading take a crate-internal base-directory seam, and the tests go through it.
- Rust tests never change the process environment. API key fallbacks, `${VAR}` expansion, `OTEL_*` and `AWS_REGION` read through a crate-internal env-lookup seam. Spawned children get their environment through the sandbox's `env` option.
- The future returned by `create_harness(..)` is `Send`, checked at compile time.
- Live tests (`harness-ts/test/integration/`) assert on the trace and side effects, not on wording, and mirror the `toolAttempted`, `toolSucceeded` and `toolResultContains` helpers from `harness-ts/test/integration/e2e.test.ts`. Run them with `cargo test -p strands-harness --all-features -- --ignored live_`.
