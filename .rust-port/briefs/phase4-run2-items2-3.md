You are implementing Phase 4 items 2 and 3 of the Strands Rust SDK port. Work autonomously; the owner is unavailable.

Start by reading `.rust-port/briefs/phase4.md` in full: it is the Phase 4 brief and all of its reading list, rules and report format apply. Item 1 is done (see `.rust-port/STATE.md` "Current item" for its leftovers that belong to items 2 and 3, for example the cancel cases of `agent.interrupt.test.ts` and the agent-as-tool cancel cases).

Scope for this run: item 2 (`CancelHandle`, `CancelOptions`, `CancelSignal`, TS cancel semantics) and then item 3 (drop guard history repair, deferred cleanup on the next call, `shutdown()`, drop warning). Stop after item 3 is committed, ticked and pushed. Don't start item 4.

Environment: repo `/mnt/workspace/harness-sdk`, branch `feat/rust-sdk` (never switch). Export `RUSTUP_HOME=/mnt/workspace/.rust/rustup CARGO_HOME=/mnt/workspace/.rust/cargo PATH=/mnt/workspace/.rust/cargo/bin:$PATH CARGO_TARGET_DIR=/mnt/workspace/.cache/cargo-target`. Gates per item: `GATE_OUT=/mnt/workspace/gates-phase4 .rust-port/gates.sh`. Another implementer works in parallel in a separate worktree on the model providers (`strands-rs/strands/src/models/{bedrock,anthropic,openai}*`); don't edit those paths. If cargo prints "Blocking waiting for file lock", just wait.

Decision numbers: use P-90 upwards in `.rust-port/DECISIONS-pending.md` (the provider implementer uses P-150 upwards in a staging file).

Commit with `git commit --no-verify` after `.rust-port/secret-scan.sh` prints `scan: clean`; push with `git push fork feat/rust-sdk:feat/rust-sdk-run-20261115` after every commit (never force, never any other branch). Nobody else commits to this worktree while you run. Update the "Current item" line of `.rust-port/STATE.md` when you tick an item.

Report: per item what was built, test counts, TS specs ported and remaining gaps with reasons, P numbers, TS bugs, commits, anything undone.
