# State

## Where we are

- Status: in progress (fresh start on 2026-11-15; second session started 2026-11-15 19:07 UTC)
- Current phase: Phase 4 (Phases 0-3 complete, review gates closed; 1084 tests, `gates.sh full` green at 743a1880d)
- Current item: Phase 4 phase review gate and fixes (run by the lead). Items 1-8 are done and gated (`gates.sh` green at item 8, 3036 tests all-features): interrupts and resume (P-89), cancel handles and checkpoints (P-90), the drop guard with deferred cleanup, `shutdown()` and the drop warning (P-91), snapshots (P-92), the `Plugin` trait and registration order (P-93), middleware stages and phases with delegation on `ExecuteToolStage` (P-94), continuations and background tasks with `loadSnapshot`'s task check (P-95), conversation managers with the token-usage middleware, pinning helpers and the stateful-model interplay (P-96). Leftovers for later items: the session-manager cases of `agent-as-tool.interrupt.test.ts` (1), `agent-as-tool.test.ts` and `agent.stateful-model.test.ts` (3) (Phase 6); the router, memory, context manager and session manager slots of the plugin registration order join with their subsystems; `agentic-context.test.ts` (30, the agentic tools) and installing the token-usage middleware for `contextManager: 'agentic'` join with the context manager (Phase 6). Carried-over leftovers: the 8 cycle-duration and `_estimateInputTokens` cases of `agent.test.ts`; the `InterruptState` unit cases of `interrupt.test.ts` (14) aren't ported one to one; test renames to TS titles are tracked in Phase 11 (P-88)
- Phase base commits: Phase 0 = 7296caba7; Phase 1 = d2cce53e2 (Phase 0 review diff 7296caba7..d2cce53e2; Phase 0 fixes landed in 95c800b0e..0e4b20905); Phase 2 = 0e4b20905 (Phase 1 review diff d2cce53e2..0e4b20905); Phase 3 = ef33e6992 (Phase 2 review diff 0e4b20905..ef33e6992); Phase 4 = 743a1880d (Phase 3 review diff ef33e6992..a393289a2; fixes 6b0380c62..743a1880d)

## Credentials

- ANTHROPIC_API_KEY: missing
- OPENAI_API_KEY: missing
- AWS: ok (default chain, region set in env)

## Environment notes

- The sandbox is ephemeral between sessions; only `/mnt/workspace` survives a resume and a fresh session may start empty. The branch is the system of record.
- The branch is persisted by pushing `feat/rust-sdk` to the remote `fork` (`https://github.com/strandly-the-agent/harness-sdk.git`) as `feat/rust-sdk-run-20261115`. Never force-push; never push anywhere else. Older branches on that remote (`feat/rust-sdk`, `feat/rust-sdk-fresh`, `feat/rust-sdk-prev-20261115`) are unrelated earlier attempts and are not used.
- Rust toolchains live in `/mnt/workspace/.rust` (`RUSTUP_HOME=/mnt/workspace/.rust/rustup`, `CARGO_HOME=/mnt/workspace/.rust/cargo`, add `$CARGO_HOME/bin` to `PATH`). Stable is 1.100.0; MSRV toolchain is 1.96.0. cargo-deny 0.20.2 is installed there. Node 22.23.4 is in `/mnt/workspace/.node/bin` (`npm ci` and `npm run build` done at the repo root; `node_modules/` and `strands-ts/dist/` are untracked build output).
- `/tmp` does not survive a restart, so the spec export, donor and reference clones live under `/mnt/workspace/` and are symlinked: `/tmp/ts-spec -> /mnt/workspace/ts-spec`, `/tmp/donor-strandly -> /mnt/workspace/donor-strandly`, `/tmp/ref-witcherish -> /mnt/workspace/ref-witcherish`. Recreate the symlinks (and the clones, per PROMPT.md section 9.1) if missing.
- Host: Linux aarch64, 4 cores.

## Open threads

- Subagents are cut off after 2 hours (`Timed out after 7200s`) and occasionally fail with `maximum recursion depth exceeded`; scope each brief to fit, ask for commits after each step, and check the worktree for uncommitted work after a failure.
- Disk: several worktree target dirs at once fill the 40 GB volume. Build with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`, delete a worktree's target dir as soon as it is integrated, and prune stale `debug/build/strands*` dirs older than 15 minutes.

- Parallel tracks (second session): Phase 4 runs in this checkout; Phase 5 providers and Phase 9 MCP items 1-4 run in separate worktrees (`/mnt/workspace/wt-phase5c` on `wip/phase5c`, `/mnt/workspace/wt-phase9` on `wip/phase9`, never pushed) and are cherry-picked onto `feat/rust-sdk` between Phase 4 runs. Their logs are staged under `.rust-port/staging/<track>/` and folded into the shared logs at integration. P-number ranges: Phase 4 below P-150, Bedrock P-150.., Anthropic P-180.., OpenAI/Mantle P-200.., MCP P-230... Because later-phase work lands during Phase 4, phase review diffs are path-scoped (Phase 4 excludes `models/{bedrock,anthropic,openai}*` and `mcp*`; Phase 5 and Phase 9 reviews take only their paths). If a session ends with a worktree branch unintegrated, check whether `/mnt/workspace/wt-*` survived and cherry-pick it; otherwise redo that item.

- Disk: `/mnt/workspace` is 40 GB. The shared cargo target (`CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target`) grows past 28 GB when reviewers build from separate worktrees; give reviewers that same target dir, and when free space drops under about 5 GB delete `$CARGO_TARGET_DIR/debug` (a full rebuild takes a few minutes).
- Subagents occasionally fail with a transient model error; check `git status` for uncommitted work, back it up under `/mnt/workspace/wip-backup/`, and rerun with a narrower scope.

- Disk: `/mnt/workspace` (40G) filled up on 2026-11-15; the shared `CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target` held 30G of debug output. Clearing `debug/incremental` freed 7.6G; `debug/build` (22G) is the next candidate if it fills again.

- CI workflows can't be pushed under `.github/workflows/` with this environment's token; they are staged in `.rust-port/ci-workflows/` (P-13). Any later CI edits go there too.

- The reference-only repository `https://github.com/TheWitcherish/strands-harness-rs.git` returns "repository not found" (2026-11-15). It is reference-only (decision #4), so work continues without it.
- Unused-dependency cargo warnings (cargo 1.100 `unused_dependencies`) appear while crates are placeholders; they go away as modules use the dependencies.
