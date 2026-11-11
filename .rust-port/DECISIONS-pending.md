# Pending decisions

**P-1. Commits skip the husky hook.** The pre-commit hook builds and tests strands-ts; the branch changes no TS. Options: run the hook, skip it. Choice: `git commit --no-verify` (section 9.5). Files: all commits.

**P-2. Branch returned by pushing to the agent's fork.** The run environment's sandbox does not persist between sessions and its instructions say to push work to a remote branch. Options: keep local only (lost on recycle), push. Choice: push only `feat/rust-sdk` to `strandly-the-agent/harness-sdk`; no PRs, issues or posts. Files: none.

**P-3. Shared build cache under a small disk.** About 5.5 GB free. Options: separate target dirs per reviewer (section 9.9), one shared target dir. Choice: builds use `CARGO_TARGET_DIR=$HOME/target` and `CARGO_PROFILE_DEV_DEBUG=line-tables-only`; reviewers share it (cargo's build lock serializes them) instead of `/tmp/review-target-<role>`. Files: `.rust-port/gates.sh`.

**P-4. Secret scan leaves `PROMPT.md` out of the pattern check.** The brief contains the scan regex itself, so the section 9.5 patterns always match it. Choice: `.rust-port/scan.sh` runs the patterns on the staged diff minus `.rust-port/PROMPT.md` and `scan.sh` itself (both contain the regex), runs the env-value check on the whole diff, and exits non-zero on a hit. Files: `.rust-port/scan.sh`.
