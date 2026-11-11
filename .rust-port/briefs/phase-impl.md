# Phase implementer brief (Appendix E), shared by phases 1-12

Read `.rust-port/briefs/_common.md` first and follow it. Then read `.rust-port/PROMPT.md` in full; sections 3, 5, 7, 8, 9.2-9.7, 10, 11 and your phase's part of section 12 (tracker items and Details) are binding. Then `strands-rs/AGENTS.md`, `strands-rs/docs/TESTING.md` (and the `harness-rs/` equivalents for Phase 12), `.rust-port/STATE.md` and `.rust-port/DECISIONS-pending.md`.

You are implementing the phase named in your task, of the Strands Rust SDK port. Work autonomously; the owner is unavailable. Decide ambiguities, log them as P-entries (check the highest P number first), and keep going (#73).

- Scope: every unticked tracker item of your phase in `.rust-port/PLAN-TRACKER.md` except "Phase review gate and fixes" (the lead runs that). Resume from the first unticked item if some are already ticked.
- For each item follow section 9.2: read the TS source and `__tests__` at the pin (`/tmp/ts-spec/...`), read the matching donor code under `/tmp/donor-strandly/strands-rs/crates/...` and re-diff it against the pin before reusing anything, implement per sections 7 and 8, port the TS tests (9.4), `cargo fmt --all`, `.rust-port/gates.sh` green, update `PARITY.md` rows and logs, scan, commit, tick, push.
- Existing code from earlier phases is the base; build on it, don't duplicate it. If an earlier phase's shape blocks you, change it minimally and say so in your report.
- If you run low on context, commit and push what is done, tick only finished items, update `STATE.md` with exactly where you stopped, and report; the lead will resume you.
- At the end: `.rust-port/gates.sh full` green, each provider feature alone compiles (`--no-default-features --features <f>`), `STATE.md` updated (current item = this phase's review gate), committed and pushed.

Report back concisely: modules and key public types, test counts from the gate script, TS specs ported and gaps, P numbers logged, TS bugs, anything left undone, commit range, `df -h /`.
