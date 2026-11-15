You are implementing Phase 4 items 6 and 7 of the Strands Rust SDK port. Work autonomously; the owner is unavailable.

Start by reading `.rust-port/briefs/phase4.md` in full: it is the Phase 4 brief and all of its reading list, rules and report format apply. Earlier items are done; `.rust-port/STATE.md` "Current item" lists leftovers from earlier items that belong to yours (port them too), and `.rust-port/DECISIONS-pending.md` P-90 onwards records what earlier Phase 4 items decided.

Scope for this run: item 6 (middleware stages and phases, including `AgentStreamStage`) and then item 7 (background tasks, and the continuation mechanism deferred in P-78 with `agent.continuation.test.ts`). Stop after item 7 is committed, ticked and pushed. Don't start item 8.

Environment: repo `/mnt/workspace/harness-sdk`, branch `feat/rust-sdk` (never switch). Export `RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo PATH=/mnt/workspace/.rust/cargo/bin:$PATH CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target`. Gates per item: `GATE_OUT=/mnt/workspace/gates-phase4 .rust-port/gates.sh`. Another implementer works in parallel in a separate worktree on the model providers (`strands-rs/strands/src/models/{bedrock,anthropic,openai}*`); don't edit those paths. If cargo prints "Blocking waiting for file lock", just wait.

Decision numbers: continue from the highest P number below P-150 in `.rust-port/DECISIONS-pending.md` (P-150 and up are reserved for the provider implementers, whose entries the lead folds in).

Commit with `git commit --no-verify` after `.rust-port/secret-scan.sh` prints `scan: clean`; push with `git push fork feat/rust-sdk:feat/rust-sdk-run-20261115` after every commit (never force, never any other branch). Nobody else commits to this worktree while you run. Update the "Current item" line of `.rust-port/STATE.md` when you tick an item.

Report: per item what was built, test counts, TS specs ported and remaining gaps with reasons, P numbers, TS bugs, commits, anything undone.
