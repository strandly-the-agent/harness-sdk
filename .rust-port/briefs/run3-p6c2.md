Track `p6c2`: finish Phase 6 item 3 (the experimental `ContextManager`). Read `.rust-port/briefs/run3-common.md` first and follow it.

Worktree `/mnt/workspace/wt-p6c2` (branch `wip/p6c2`), `CARGO_TARGET_DIR=/mnt/workspace/.cache/ct-p6c2`, gate output `/mnt/workspace/gates-p6c2`, staging dir `.rust-port/staging/p6c2/`, decisions from P-321 (P-310..320 are in `.rust-port/staging/phase6c/decisions.md`, the previous run of this track; read that folder and the original brief `.rust-port/briefs/phase6-context-manager.md`).

State: the context manager is implemented under `strands-rs/strands/src/experimental/context_manager*` with agent and session wiring, gates green. Left to do, in this order (commit after each):
1. Port the 55 cases of `/tmp/ts-spec/strands-ts/src/context-manager/strategies/__tests__/summarize-strategy.test.ts` (fix any behaviour difference they expose against `strategies/offload/summarize*.ts`).
2. Port the remaining 11 stash cases of `session/__tests__/session-manager.test.ts` (restore from inline and external, the storage-type warning, the delete variants, `succeeds when no stash is available`); the 7 done are listed in `.rust-port/staging/phase6c/parity.md`.
3. Pass `agent_metadata` (and the session id where TS does) to `Model::summarize` as TS does (`grep -n P-304 .rust-port/DECISIONS-pending.md` and the Phase 6 note in `.rust-port/STATE.md`: "session id and agent metadata are not sent to the model yet"); implement what TS sends at the pin and test it.
4. Remove the now-stale `allow(dead_code)`/`expect(dead_code)` on the conversation-manager helpers the context manager now uses.
5. The 8 cycle-duration and `_estimateInputTokens` cases of `agent/__tests__/agent.test.ts` listed as carried over in the `agent.test.ts` row of `strands-rs/docs/PARITY.md`, if the mechanisms now exist; otherwise leave the gap reason accurate.
6. Self-review the whole `experimental/context_manager` tree against `/tmp/ts-spec/strands-ts/src/context-manager/` for texts, defaults, thresholds and tool schemas; fix differences.
Write updated parity rows (full replacement rows for the specs you touch) to `.rust-port/staging/p6c2/parity.md`. Count TS cases with `cd /mnt/workspace/harness-sdk && node strands-rs/docs/count-ts-cases.mjs <spec>`.
