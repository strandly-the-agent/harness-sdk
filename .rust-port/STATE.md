# State

## Where we are

- Status: in progress (fresh start on 2026-11-15)
- Current phase: Phase 4 (Phases 0-3 complete, review gates closed; 1084 tests, `gates.sh full` green at 743a1880d)
- Current item: Phase 4 item 1 (interrupts and resume). Brief: `.rust-port/briefs/phase4.md`. Carried-over leftovers: the 8 cycle-duration and `_estimateInputTokens` cases of `agent.test.ts`; continuations wait for background tasks (P-78); agent-as-tool cancel cases, sub-agent interrupt propagation and agent.hook.test.ts :652 wait for Phase 4; test renames to TS titles are tracked in Phase 11 (P-88)
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

- Disk: `/mnt/workspace` is 40 GB. The shared cargo target (`CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target`) grows past 28 GB when reviewers build from separate worktrees; give reviewers that same target dir, and when free space drops under about 5 GB delete `$CARGO_TARGET_DIR/debug` (a full rebuild takes a few minutes).
- Subagents occasionally fail with a transient model error; check `git status` for uncommitted work, back it up under `/mnt/workspace/wip-backup/`, and rerun with a narrower scope.

- Disk: `/mnt/workspace` (40G) filled up on 2026-11-15; the shared `CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target` held 30G of debug output. Clearing `debug/incremental` freed 7.6G; `debug/build` (22G) is the next candidate if it fills again.

- CI workflows can't be pushed under `.github/workflows/` with this environment's token; they are staged in `.rust-port/ci-workflows/` (P-13). Any later CI edits go there too.

- The reference-only repository `https://github.com/TheWitcherish/strands-harness-rs.git` returns "repository not found" (2026-11-15). It is reference-only (decision #4), so work continues without it.
- Unused-dependency cargo warnings (cargo 1.100 `unused_dependencies`) appear while crates are placeholders; they go away as modules use the dependencies.
