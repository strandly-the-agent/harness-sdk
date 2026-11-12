# Phase review-gate fix brief, shared by phases 1-12

Read `.rust-port/briefs/_common.md` first and follow it, then `.rust-port/PROMPT.md` in full (sections 3, 7, 8, 9.9, 10, 11 and your phase's part of section 12 matter most).

Task: the review-gate fix loop of the phase named in your task. Three independent reviewers reported findings on the phase diff; their reports are at `/tmp/review_phase<N>_{correctness,api,parity}.md`. For each finding: verify it yourself; fix every verified finding in follow-up commits (`fix(strands): ...`, `docs: ...`, `test(strands): ...`, or `fix(harness): ...` for Phase 12); if you disagree, record why with evidence. Never weaken a lint or skip a test to get green.

Disk: run `rm -rf $HOME/target` before your first build if `df -h /` shows under 2 GB free; if it drops under 700 MB mid-run, delete `$HOME/target` and rebuild, and report it.

Then write `strands-rs/docs/reviews/phase-<N>.md` (`harness-rs/docs/reviews/` for Phase 12; public file, no internal context) with a table `finding | disposition | commit` covering every finding (C1.., A1.., P1..). Run `.rust-port/gates.sh full` after the fixes; it must pass, and each provider feature alone must compile. Tick "Phase review gate and fixes" and the phase heading in `.rust-port/PLAN-TRACKER.md`, update `.rust-port/STATE.md` (current phase = next phase, first item; record the next phase base commit = your last non-`.rust-port` commit), commit and push with `.rust-port/commit.sh`.

If you run low on context, commit what is done, record in `STATE.md` which findings remain, and report.

Report back concisely: findings fixed vs disputed (ids), gate output lines, commit hashes, the next phase base commit, `df -h /`.
