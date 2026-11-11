# Phase 0 implementer brief

Read `.rust-port/briefs/_common.md` first and follow it.

You are implementing the remaining Phase 0 items of the Strands Rust SDK port. Work autonomously; the owner is unavailable. Read `.rust-port/PROMPT.md` in full (sections 3, 7, 8, 9, 10, 11 and the Phase 0 part of section 12 matter most, and section 8.1 for crate manifests and features). Setup from 9.1 is done except: `npm ci` / `npm run build` (needed for the conformance generator; check disk first and delete anything you don't need afterwards) and reading the repo conventions docs listed in 9.1 ("Then:" bullets).

Scope: every unticked Phase 0 tracker item except "Phase review gate and fixes" (the lead runs that):
- Workspace skeleton: root `Cargo.toml`, three crates, features, lints, `deny.toml`, rustfmt/clippy config
- AGENTS.md files (root layout, strands-rs, harness-rs), `docs/TESTING.md`, `docs/PARITY.md`
- CI workflows and `ci.yml` path filters (test-lint, MSRV, deny, doctests)
- Conformance fixture generator (tsx) and committed fixtures
- Run logs and `DIVERGENCES.md` files created

Donor process templates: `/tmp/donor-strandly/strands-rs/docs/TESTING.md`, `/tmp/donor-strandly/strands-rs/docs/reviews/`, donor manifests and lints. Re-check every dependency version against crates.io today; keep MSRV 1.96 valid with `--locked`.

Gates: `.rust-port/gates.sh full` must pass at the end (including msrv and deny). Commit per item, tick it in `PLAN-TRACKER.md`, push after each commit. Update `STATE.md` at the end (current item = Phase 0 review gate).

Report back (concise): what you created per item, gate output lines, P numbers logged, anything left undone or blocked, commit hashes, and `df -h /` at the end.
