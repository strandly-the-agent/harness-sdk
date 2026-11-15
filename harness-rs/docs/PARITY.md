# Parity

Every harness-ts spec file at the pinned TS baseline maps to Rust tests here. Each Rust test is named after the snake_case form of the TS `it(...)` title. A gap needs a reason.

`total` counts `it(...)`/`test(...)` call sites in the TS file (a parameterized `it.each` counts once).

## In scope

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `harness-ts/test/agent-generalist-memory.test.ts` | | 0/3 | |
| `harness-ts/test/agent.test.ts` | | 0/96 | |
| `harness-ts/test/builtin-tools.test.ts` | | 0/20 | |
| `harness-ts/test/config.test.ts` | | 0/31 | |
| `harness-ts/test/internal.test.ts` | | 0/2 | |
| `harness-ts/test/interventions.test.ts` | | 0/11 | |
| `harness-ts/test/memory.test.ts` | | 0/19 | |
| `harness-ts/test/models.test.ts` | | 0/111 | |
| `harness-ts/test/openai-cache.test.ts` | | 0/1 | |
| `harness-ts/test/plugins/environment.test.ts` | | 0/11 | |
| `harness-ts/test/plugins/todos.test.ts` | | 0/9 | |
| `harness-ts/test/prompt.test.ts` | | 0/3 | |
| `harness-ts/test/sandbox.test.ts` | | 0/3 | |
| `harness-ts/test/telemetry.test.ts` | | 0/9 | |
| `harness-ts/test/tools/file-tools.test.ts` | | 0/18 | |
| `harness-ts/test/tools/programmatic-tool-caller.test.ts` | | 0/42 | |
| `harness-ts/test/tools/shell.test.ts` | | 0/3 | |
| `harness-ts/test/tools/subagent.test.ts` | | 0/53 | |
| `harness-ts/test/tools/web-fetch.test.ts` | | 0/18 | |
| `harness-ts/test/tools/web-search.test.ts` | | 0/11 | |

## Live checklist

| TS spec | reason |
|---|---|
| `harness-ts/test/integration/caching.test.ts` | live checklist (run as live tests, not ported as unit tests) |
| `harness-ts/test/integration/e2e.test.ts` | live checklist (run as live tests, not ported as unit tests) |
| `harness-ts/test/integration/web-search.test.ts` | live checklist (run as live tests, not ported as unit tests) |
