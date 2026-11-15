# Parity

Every spec file of the TypeScript harness this port tracks (`harness-ts/test/`) maps to Rust tests here. Each Rust test is named after the snake_case form of the TS `it(...)` title. A gap needs a reason.

`total` is the number of test cases the TS file declares: every call rooted at `it` or `test`, including `.only`, `.skip`, `.todo`, `.concurrent` and the curried `it.each(..)(..)`, `it.for(..)(..)`, `it.skipIf(..)(..)` and `it.runIf(..)(..)` forms. A parameterized case counts once; skipped and todo cases count. Recount from the repo root (after `npm ci`) with:

```bash
node strands-rs/docs/count-ts-cases.mjs harness-ts/test
```

`*.test.browser.ts` specs don't apply to Rust.

## In scope

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `harness-ts/test/agent-generalist-memory.test.ts` | | 0/3 | |
| `harness-ts/test/agent.test.ts` | | 0/100 | |
| `harness-ts/test/builtin-tools.test.ts` | | 0/22 | |
| `harness-ts/test/config.test.ts` | | 0/32 | |
| `harness-ts/test/internal.test.ts` | | 0/2 | |
| `harness-ts/test/interventions.test.ts` | | 0/11 | |
| `harness-ts/test/memory.test.ts` | | 0/19 | |
| `harness-ts/test/models.test.ts` | | 0/111 | |
| `harness-ts/test/openai-cache.test.ts` | | 0/2 | |
| `harness-ts/test/plugins/environment.test.ts` | | 0/11 | |
| `harness-ts/test/plugins/todos.test.ts` | | 0/9 | |
| `harness-ts/test/prompt.test.ts` | | 0/3 | |
| `harness-ts/test/sandbox.test.ts` | | 0/3 | |
| `harness-ts/test/telemetry.test.ts` | | 0/9 | |
| `harness-ts/test/tools/file-tools.test.ts` | | 0/19 | |
| `harness-ts/test/tools/programmatic-tool-caller.test.ts` | | 0/43 | |
| `harness-ts/test/tools/shell.test.ts` | | 0/3 | |
| `harness-ts/test/tools/subagent.test.ts` | | 0/53 | |
| `harness-ts/test/tools/web-fetch.test.ts` | | 0/19 | |
| `harness-ts/test/tools/web-search.test.ts` | | 0/11 | |

## Live checklist

| TS spec | reason |
|---|---|
| `harness-ts/test/integration/caching.test.ts` | live checklist (run as live tests, not ported as unit tests) |
| `harness-ts/test/integration/e2e.test.ts` | live checklist (run as live tests, not ported as unit tests) |
| `harness-ts/test/integration/web-search.test.ts` | live checklist (run as live tests, not ported as unit tests) |
