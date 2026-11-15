# Test parity - Strands harness for Rust

This file tracks how the TypeScript harness test suite (`harness-ts/test/`), the behavioural spec for `strands-harness`, is ported to Rust. Paths are relative to `harness-ts/test/`. Every spec in this suite is in scope. See [../../strands-rs/docs/TESTING.md](../../strands-rs/docs/TESTING.md) for how tests are written and named.

Each row is one TypeScript spec file at commit `7296caba7`, the TypeScript revision this port follows. `Rust tests` names the Rust test module or file that ports it. `ported/total` counts the TypeScript cases ported against the cases in the file: `total` counts the `it(...)` and `test(...)` call sites, including `it.each`, `it.for`, `it.skipIf` and `it.runIf`. A parameterised case (`it.each`, `it.for`) counts as one until it is ported; its row then counts each table entry. A case that is not ported is listed under `gaps` with its reason (out of scope, a TypeScript-only mechanism, or deprecated). Rows marked `n/a` are out of scope for v0.1 as a whole.

## Factory, config and models

Phase 12.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `agent-generalist-memory.test.ts` | - | 0/3 | - |
| `agent.test.ts` | - | 0/100 | - |
| `config.test.ts` | - | 0/32 | - |
| `internal.test.ts` | - | 0/2 | - |
| `models.test.ts` | - | 0/111 | - |
| `openai-cache.test.ts` | - | 0/2 | - |
| `prompt.test.ts` | - | 0/3 | - |

## Built-in features

Phase 12.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `builtin-tools.test.ts` | - | 0/22 | - |
| `interventions.test.ts` | - | 0/11 | - |
| `memory.test.ts` | - | 0/19 | - |
| `sandbox.test.ts` | - | 0/3 | - |
| `telemetry.test.ts` | - | 0/9 | - |

## Plugins

Phase 12.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `plugins/environment.test.ts` | - | 0/11 | - |
| `plugins/todos.test.ts` | - | 0/9 | - |

## Tools

Phase 12.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `tools/file-tools.test.ts` | - | 0/19 | - |
| `tools/programmatic-tool-caller.test.ts` | - | 0/43 | - |
| `tools/shell.test.ts` | - | 0/3 | - |
| `tools/subagent.test.ts` | - | 0/53 | - |
| `tools/web-fetch.test.ts` | - | 0/19 | - |
| `tools/web-search.test.ts` | - | 0/11 | - |

## Integration (live checklist)

Phase 12. These specs call real models; they are ported as `live_*` tests, and results are recorded in `docs/reviews/live.md`.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `integration/caching.test.ts` | - | 0/2 | - |
| `integration/e2e.test.ts` | - | 0/11 | - |
| `integration/web-search.test.ts` | - | 0/1 | - |
