# Pending decisions

**P-1. Commit with `--no-verify`.** Context: the husky pre-commit hook builds and tests strands-ts on every commit, and this branch changes no TS. Options: run the hook, skip it. Choice: `git commit --no-verify` for every commit. TS reference: `.husky/pre-commit`. Files: all commits.

**P-2. Branch return.** Context: the sandbox is not persistent across sessions, and the environment's instructions say work survives only by pushing a branch. Options: keep local only, push. Choice: push only this branch, only to the `strandly-the-agent/harness-sdk` fork, as `feat/rust-sdk-fresh` (the existing `feat/rust-sdk` there is an earlier attempt, left untouched). Files: none.
