You are finishing Phase 9 items 1-4 (MCP) of the Strands Rust SDK port. A previous implementer ran out of time after four commits on `wip/phase9`; you continue from its state. Work autonomously; the owner is unavailable.

Read first: the original brief `/mnt/workspace/wt-phase9/.rust-port/briefs/phase9-mcp.md` (all of its rules, paths and constraints apply to you), then `git -C /mnt/workspace/wt-phase9 log --oneline 207de11d1..HEAD`, the staged logs in `/mnt/workspace/wt-phase9/.rust-port/staging/phase9/`, and the module `strands-rs/strands/src/mcp*`.

State you inherit: the worktree `/mnt/workspace/wt-phase9` has an uncommitted change in `strands-rs/strands/src/mcp/tests.rs` (two new streamable-HTTP reconnection tests). `staging/phase9/parity.md` is missing.

Scope, in order (commit after each step; your time is limited, so keep moving and commit early):
1. Check the uncommitted tests compile and pass; fix or drop them; commit.
2. Map every `it(...)` of `/tmp/ts-spec/strands-ts/src/mcp/__tests__/{client,client-annotations,client-progress}.test.ts` (and the MCP config-loading tests: `rg -l loadServers /tmp/ts-spec/strands-ts/src --glob '*test*'`) to a Rust test. Port the missing applicable cases (snake_case TS titles). Commit.
3. Write `.rust-port/staging/phase9/parity.md` with rows for `strands-rs/docs/PARITY.md` (columns `TS spec | Rust tests | ported/total | gaps (with reason)`; see the existing rows for those files in `strands-rs/docs/PARITY.md` and reuse their exact first-column text), and make sure `decisions.md`, `divergences.md`, `ts-bugs.md` are complete. Commit.
4. Run `GATE_OUT=/mnt/workspace/gates-phase9 .rust-port/gates.sh` from the worktree root with `CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target-p9 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=0 CARGO_PROFILE_TEST_DEBUG=0` (always set these; never build into another target dir). Fix everything; commit.

Never kill processes by name (other agents run cargo and gates.sh concurrently); only kill PIDs you started. If `df -h /mnt/workspace` shows under 3 GB free, delete only `/mnt/workspace/.cache/cargo-target-p9/debug/build/strands*` directories older than 15 minutes.

Report (concise): test counts, cases ported per TS spec and gaps with reasons, P numbers, TS bugs, shared-file edits made by you or your predecessor (`git diff 207de11d1..HEAD --stat -- ':!strands-rs/strands/src/mcp*' ':!.rust-port'`), commit hashes, anything undone.
