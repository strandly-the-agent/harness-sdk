# Agent Development Guide - Strands Rust harness

This guide covers `harness-rs/`, the Rust port of the Strands harness. Follow the root [AGENTS.md](../AGENTS.md) and the Rust SDK guide [strands-rs/AGENTS.md](../strands-rs/AGENTS.md); this file adds what is specific to the harness.

## Purpose

`strands-harness` builds a ready-to-run agent with `create_harness`, with the same defaults, prompts, built-in tools, plugins and on-disk layout as the TypeScript harness in `harness-ts/`. harness-ts is canonical for behaviour and text; the Rust API re-cases its names and follows the SDK's API rules.

## Layout

```
harness-rs/
├── strands-harness/
│   ├── src/        # lib, agent, config, defaults, models, prompt, interventions, memory, telemetry,
│   │               # builtin_tools, sandbox, tools/, plugins/
│   └── tests/
└── docs/           # PARITY.md, DIVERGENCES.md, reviews/
```

The crate is a member of the root Cargo workspace and depends on `strands` with the `mcp` feature. Its `bedrock` (default), `anthropic`, `openai` and `otel` features forward to `strands`. `monty`, which runs the programmatic tool caller, is a dependency of this crate only.

## Gates

The workspace gates in [strands-rs/AGENTS.md](../strands-rs/AGENTS.md#gates) cover this crate. Harness changes also run in CI whenever `strands-rs/` changes, because the harness depends on the SDK.

## Code rules

The SDK code rules, lints and `#[non_exhaustive]` policy apply unchanged. In addition:

- Every built-in tool and plugin goes through the agent's sandbox (`ctx.agent().sandbox()`), never the host filesystem or process APIs directly.
- `HarnessAgentOptions` follows the `AgentConfig` pattern: public fields, `Default`, a builder, and every TS field. Every config key either changes behaviour or produces the TS "unsupported" error; nothing is accepted and ignored.
- Telemetry setup is explicit and idempotent. Constructors have no process-global side effects.
- Subagents reuse the parent's connected MCP clients, get recall-only memory, and keep preset order.
- Names follow harness-ts. The only rename is the subagent `Option` type, which is `ChoiceOption` because `Option` clashes with the Rust prelude.

## Wire and disk compatibility

- Default sessions live at `./.agent` under the `sessions` namespace, with the same paths as harness-ts. Rust writes compact JSON; both SDKs read either form.
- System prompt assembly, tool descriptions and the `read`, `edit` and `todos` text match harness-ts byte for byte.
- User-visible differences from harness-ts are recorded in [docs/DIVERGENCES.md](docs/DIVERGENCES.md).

## Testing

Follow [strands-rs/docs/TESTING.md](../strands-rs/docs/TESTING.md). harness-ts tests that `process.chdir` into a temp dir are ported through a crate-internal base-directory seam; Rust tests never change the process working directory or env. [docs/PARITY.md](docs/PARITY.md) maps each harness-ts spec to its Rust tests; `harness-ts/test/integration/` is the live checklist, and live results are recorded in [docs/reviews/live.md](docs/reviews/live.md).
