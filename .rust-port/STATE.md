# State

## Where we are

- Status: in progress (fresh start on 2026-11-15; second session 2026-11-15 19:07 to 2026-11-16 02:30 UTC)
- Current phase: parallel tracks. Complete with closed review gates: Phases 0-3, 5 and 8. Phase 4: items 1-8 done; review gate open (fixes landed except api-4 and api-5, see `strands-rs/docs/reviews/phase-4.md`). Phase 6: items 1-2 done; item 3 (`ContextManager`) in progress as WIP patches. Phase 9: items 1-5 done; review gate open (21 findings in `.rust-port/reviews/phase9-review-*.md`; a fix run is in progress as WIP patches). Phases 7 and 10-13 not started. `gates.sh` green at fad64622b (3244 tests all-features); `gates.sh full` green at 7cd6f6ab4.
- Next steps, in order: (1) apply the WIP patches in `.rust-port/wip/` (see below) or, if the worktrees `/mnt/workspace/wt-phase9fix` and `/mnt/workspace/wt-phase6c` survived, cherry-pick their branches `wip/phase9fix` and `wip/phase6c` past the commits already in `.rust-port/wip/`; fold their `.rust-port/staging/<track>/` logs into the shared logs and delete the staging folder; (2) finish the Phase 9 fixes (brief `.rust-port/briefs/phase9-fix.md`; dispositions so far in the staged `dispositions.md`), write `strands-rs/docs/reviews/phase-9.md`, run `gates.sh full`, tick Phase 9; (3) fix Phase 4 api-4 and api-5, update `phase-4.md`, run `gates.sh full`, tick Phase 4; (4) finish Phase 6 item 3 (brief `.rust-port/briefs/phase6-context-manager.md`), then items 4-6 and the Phase 6 gate; (5) Phase 7, Phase 10, then 11-13. Review reports referenced by briefs as `/mnt/workspace/reviews/...` are copied in `.rust-port/reviews/`. Phase 4 leftovers that wait on Phase 6: the session-manager cases of `agent-as-tool.interrupt.test.ts`, `agent-as-tool.test.ts` and `agent.stateful-model.test.ts` (3); `agentic-context.test.ts` (30) and the token-usage middleware for `contextManager: 'agentic'` join with the context manager; the router, memory and context manager plugin slots join with their subsystems. Carried-over: the 8 cycle-duration and `_estimateInputTokens` cases of `agent.test.ts`; the `InterruptState` unit cases of `interrupt.test.ts` (14); test renames to TS titles in Phase 11 (P-88); session id and agent metadata are not sent to the model yet (Phase 6 note); MCP `mcp`/`mcp-v2` live rows wait for the Phase 11 live sweep.
- Phase base commits: Phase 0 = 7296caba7; Phase 1 = d2cce53e2 (Phase 0 review diff 7296caba7..d2cce53e2; Phase 0 fixes landed in 95c800b0e..0e4b20905); Phase 2 = 0e4b20905 (Phase 1 review diff d2cce53e2..0e4b20905); Phase 3 = ef33e6992 (Phase 2 review diff 0e4b20905..ef33e6992); Phase 4 = 743a1880d (Phase 3 review diff ef33e6992..a393289a2; fixes 6b0380c62..743a1880d); Phase 5 = review of the provider paths of d35634b1b..af5f294da, fixes a1384e975..d946e3485 and f7b30cc83; Phase 8 review diff f7b30cc83..338f7ff8f; Phase 9 review paths `mcp*` and `vended_tools/mcp_router*` at c04b26898; Phase 4 review diff 743a1880d..f5e53f220 excluding the later-phase paths

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
- Disk: several worktree target dirs at once fill the 40 GB volume; put worktree and reviewer target dirs on `/tmp` (the 38 GB root overlay, lost on restart, which is fine for caches) and keep `/mnt/workspace/.cache/cargo-target` for the main checkout. `/mnt/workspace/.cache/cargo-target/tests/trybuild` is a symlink into `/tmp/ct-tests` this session; if the link dangles after a restart, delete it. Build with `CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0`, delete a worktree's target dir as soon as it is integrated, and prune stale `debug/build/strands*` dirs older than 15 minutes.

- Parallel tracks (second session): Phase 4 runs in this checkout; Phase 5 providers and Phase 9 MCP items 1-4 run in separate worktrees (`/mnt/workspace/wt-phase5c` on `wip/phase5c`, `/mnt/workspace/wt-phase9` on `wip/phase9`, never pushed) and are cherry-picked onto `feat/rust-sdk` between Phase 4 runs. Their logs are staged under `.rust-port/staging/<track>/` and folded into the shared logs at integration. P-number ranges: Phase 4 below P-150, Bedrock P-150.., Anthropic P-180.., OpenAI/Mantle P-200.., MCP P-230... Because later-phase work lands during Phase 4, phase review diffs are path-scoped (Phase 4 excludes `models/{bedrock,anthropic,openai}*` and `mcp*`; Phase 5 and Phase 9 reviews take only their paths). If a session ends with a worktree branch unintegrated, its commits are saved as patches under `.rust-port/wip/<track>/` (see the README there); a surviving `/mnt/workspace/wt-*` worktree may hold newer commits.

- Disk: `/mnt/workspace` is 40 GB. The shared cargo target (`CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target`) grows past 28 GB when reviewers build from separate worktrees; give reviewers that same target dir, and when free space drops under about 5 GB delete `$CARGO_TARGET_DIR/debug` (a full rebuild takes a few minutes).
- Subagents occasionally fail with a transient model error; check `git status` for uncommitted work, back it up under `/mnt/workspace/wip-backup/`, and rerun with a narrower scope.

- Disk: `/mnt/workspace` (40G) filled up on 2026-11-15; the shared `CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target` held 30G of debug output. Clearing `debug/incremental` freed 7.6G; `debug/build` (22G) is the next candidate if it fills again.

- CI workflows can't be pushed under `.github/workflows/` with this environment's token; they are staged in `.rust-port/ci-workflows/` (P-13). Any later CI edits go there too.

- The reference-only repository `https://github.com/TheWitcherish/strands-harness-rs.git` returns "repository not found" (2026-11-15). It is reference-only (decision #4), so work continues without it.
- Unused-dependency cargo warnings (cargo 1.100 `unused_dependencies`) appear while crates are placeholders; they go away as modules use the dependencies.
