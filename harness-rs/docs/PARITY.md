# Test parity - Strands harness for Rust

This file tracks how the TypeScript harness test suite (`harness-ts/test/`), the behavioural spec for `strands-harness`, is ported to Rust. Paths are relative to `harness-ts/test/`. Every spec in this suite is in scope. See [../../strands-rs/docs/TESTING.md](../../strands-rs/docs/TESTING.md) for how tests are written and named.

Each row is one TypeScript spec file at commit `7296caba7`, the TypeScript revision this port follows. `Rust tests` names the Rust test module or file that ports it. `ported/total` counts the TypeScript cases ported against the cases in the file: `total` counts the `it(...)` and `test(...)` call sites, including `it.each`, `it.for`, `it.skipIf` and `it.runIf`. A parameterised case (`it.each`, `it.for`) counts as one until it is ported; its row then counts each table entry. A case that is not ported is listed under `gaps` with its reason (out of scope, a TypeScript-only mechanism, or deprecated). Rows marked `n/a` are out of scope for v0.1 as a whole.

## Factory, config and models

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `agent-generalist-memory.test.ts` | - | 0/3 | - |
| `agent.test.ts` | - | 0/100 | `enables web_search when explicitly selected on a supported provider`: out of scope, it uses the Google provider, which is not in v0.1 |
| `config.test.ts` | - | 0/32 | - |
| `internal.test.ts` | - | 0/2 | - |
| `models.test.ts` | - | 0/111 | `maps google auto to the high thinking level`, `uses the google model id directly`, `adds the google googleSearch built-in tool for web_search`, `omits google built-in tools when web_search is off`, `does not warn when caching is supported for litellm`, `builds litellm with explicit caching and sets no cacheConfig`, and the `google/` assertions and table rows in `rejects a level the provider does not support`, the thinking-level table and `is true for openai, google and bedrock-mantle`: out of scope, the Google and LiteLLM providers are not in v0.1 |
| `openai-cache.test.ts` | - | 0/2 | - |
| `prompt.test.ts` | - | 0/3 | - |

## Built-in features

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `builtin-tools.test.ts` | - | 0/22 | - |
| `interventions.test.ts` | - | 0/11 | - |
| `memory.test.ts` | - | 0/19 | - |
| `sandbox.test.ts` | - | 0/3 | - |
| `telemetry.test.ts` | - | 0/9 | - |

## Plugins

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `plugins/environment.test.ts` | - | 0/11 | - |
| `plugins/todos.test.ts` | - | 0/9 | - |

## Tools

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `tools/file-tools.test.ts` | - | 0/19 | - |
| `tools/programmatic-tool-caller.test.ts` | - | 0/43 | - |
| `tools/shell.test.ts` | - | 0/3 | - |
| `tools/subagent.test.ts` | - | 0/53 | - |
| `tools/web-fetch.test.ts` | - | 0/19 | - |
| `tools/web-search.test.ts` | - | 0/11 | - |

## Integration (live checklist)

These specs call real models; they are ported as `live_*` tests, and results are recorded in `docs/reviews/live.md`.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `integration/caching.test.ts` | - | 0/1 | - |
| `integration/e2e.test.ts` | - | 0/10 | - |
| `integration/web-search.test.ts` | - | 0/1 | - |
