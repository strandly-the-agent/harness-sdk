# Parity - Rust harness

This file maps each TypeScript spec file in `harness-ts/` to the Rust tests that port it. The TypeScript tests are the behavioural spec (see [strands-rs/docs/TESTING.md](../../strands-rs/docs/TESTING.md)).

Columns: `TS spec` is the TypeScript test file, `Rust tests` is where its ported tests live, `ported/total` counts ported cases against the TypeScript `it(...)` and `test(...)` cases in that file (an `it.each` table counts as one case), and `gaps` gives the reason for every case that isn't ported. `harness-ts/test/fixtures/config-values.ts` holds shared config values for `config.test.ts` and is ported with it.

Summary: 20 spec files with 485 cases. The live checklist has 3 files with 12 cases.

## Unit specs

Spec files under `harness-ts/test/`.

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `harness-ts/test/agent-generalist-memory.test.ts` | - | 0/3 | - |
| `harness-ts/test/agent.test.ts` | - | 0/100 | - |
| `harness-ts/test/builtin-tools.test.ts` | - | 0/22 | - |
| `harness-ts/test/config.test.ts` | - | 0/32 | - |
| `harness-ts/test/internal.test.ts` | - | 0/2 | - |
| `harness-ts/test/interventions.test.ts` | - | 0/11 | - |
| `harness-ts/test/memory.test.ts` | - | 0/19 | - |
| `harness-ts/test/models.test.ts` | - | 0/111 | - |
| `harness-ts/test/openai-cache.test.ts` | - | 0/2 | - |
| `harness-ts/test/plugins/environment.test.ts` | - | 0/11 | - |
| `harness-ts/test/plugins/todos.test.ts` | - | 0/9 | - |
| `harness-ts/test/prompt.test.ts` | - | 0/3 | - |
| `harness-ts/test/sandbox.test.ts` | - | 0/3 | - |
| `harness-ts/test/telemetry.test.ts` | - | 0/9 | - |
| `harness-ts/test/tools/file-tools.test.ts` | - | 0/19 | - |
| `harness-ts/test/tools/programmatic-tool-caller.test.ts` | - | 0/43 | - |
| `harness-ts/test/tools/shell.test.ts` | - | 0/3 | - |
| `harness-ts/test/tools/subagent.test.ts` | - | 0/53 | - |
| `harness-ts/test/tools/web-fetch.test.ts` | - | 0/19 | - |
| `harness-ts/test/tools/web-search.test.ts` | - | 0/11 | - |

## Live checklist

Integration tests under `harness-ts/test/integration/`, ported as `live_*` tests (see [strands-rs/docs/TESTING.md](../../strands-rs/docs/TESTING.md#live-tests)).

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `harness-ts/test/integration/caching.test.ts` | - | 0/1 | - |
| `harness-ts/test/integration/e2e.test.ts` | - | 0/10 | - |
| `harness-ts/test/integration/web-search.test.ts` | - | 0/1 | - |
