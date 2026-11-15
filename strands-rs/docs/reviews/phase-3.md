# Phase 3 review: agent loop, hooks and tools

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed `ef33e6992..a393289a2`. All gates passed at review time.

| Finding | Disposition | Commit |
|---|---|---|
| Correctness 1: a dropped stream leaves the structured-output tool registered, so the next structured call fails | Fixed: the next run removes a stale structured-output tool it registered itself; the drop guard will subsume this | `6b0380c62` |
| Correctness 2: tool progress events run ahead of the consumer | Fixed: each progress update waits for the consumer, as the before and after events do | `6b0380c62` |
| Correctness 3: blank-prompt check uses Rust `trim` | Fixed: JS-compatible trim | `6b0380c62` |
| Correctness 4: tool-returned numbers formatted the Rust way | Fixed: JS number formatting | `6b0380c62` |
| Correctness 5: delegated JSON output formatted the Rust way | Fixed: JS number formatting | `6b0380c62` |
| Correctness 6: a panic while building a tool's stream escapes a direct tool call | Fixed: caught and returned as an error | `6b0380c62` |
| API 1, 2, 8: hook traits documented as sealed are open; internal slot types reachable; unhelpful error for unannotated hook closures | Fixed: sealed, private, diagnostic messages; compile-fail tests | `08467469f` |
| API 3, 4, 5, 9, 12: builder state module, `set_model`, `From<Arc<T: Tool>>`, `From<&String>`, `InvokeLimits` builder | Fixed | `08467469f` |
| API 6: macros break when the dependency is renamed | Fixed: `crate = "..."` option on `#[tool]` and `#[tools]` | `194a95e42` |
| API 7: `ToolError` doesn't convert into boxed errors | Fixed | `08467469f` |
| API 10: `LocalAgent::metrics()` missing | Fixed | `08467469f` |
| API 11: `AgentTrace` exposes a pre-1.0 time crate | Fixed: `SystemTime` | `08467469f` |
| API 13-16, 19: stale or missing rustdoc, `printer(false)` undocumented, `Hook` example path, `# Errors` sections, no crate-root example | Fixed | `08467469f` |
| API 17: TS `toolUsage` and `accumulatedData` have no listed Rust form | Mapped to existing accessors; recorded as a divergence | `08467469f` |
| API 18: hook `attempt_count` type differs from the retry context | Fixed: `NonZeroU32` | `08467469f` |
| Parity 1, 7: missing event and hook tests | Fixed: tests added | `743a1880d` |
| Parity 2-6: PARITY rows with stale or missing reasons | Fixed | `743a1880d` |
| Parity 8: nine portable `agent.test.ts` cases not ported | Fixed | `743a1880d` |
| Parity 9: most test names don't match the TS titles | Deferred to the Phase 11 parity sweep (tracked there, with a mechanical title-mapping check); new tests use TS titles | `743a1880d` |
| Parity 10: tool-name length counted in characters rather than UTF-16 code units | Fixed | `743a1880d` |
