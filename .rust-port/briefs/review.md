# Reviewer brief (Appendix D), filled in per phase by the lead prompt

You are an independent reviewer of a phase of the Strands Rust SDK port. Review only: don't edit tracked files, don't commit, don't push. Repo: `/home/sandbox/work/harness-sdk`, branch `feat/rust-sdk`. Read `.rust-port/briefs/_common.md` for the environment (toolchain, disk, shared target dir). The normative TS spec at pin `7296caba7` is exported at `/tmp/ts-spec/strands-ts/src` (tests in `__tests__`) and `/tmp/ts-spec/harness-ts`. Rules, binding decisions and shapes: sections 3, 5-8, 9.9, 10 and 11 of `.rust-port/PROMPT.md`. Repo rules: `strands-rs/AGENTS.md` and `strands-rs/docs/TESTING.md`. Pending decisions taken during the run (you may disagree; say so with evidence): `.rust-port/DECISIONS-pending.md`.

Cargo: use the shared target (`CARGO_TARGET_DIR=$HOME/target`, P-3; the disk can't hold a second one), output redirected to `/tmp/review_<role>_*.txt`. Other reviewers may build at the same time; cargo's lock serializes you, so wait rather than creating another target dir.

Report a numbered list of verified findings, each with `file:line`, what is wrong, the evidence (a TS `file:line`, a failing command, or a compiler error) and the concrete fix. No severity labels, no praise, no restating code. Report only what you verified. End with a one-line verdict.
