# Pending decisions

Format: **P-N. Title.** Context. Options. Choice. TS reference (file:line). Files.

**P-1. Commit with `--no-verify`.** The husky pre-commit hook installed by `npm ci` builds and tests strands-ts on every commit, and this branch changes no TypeScript. Options: run the hook on every commit, or skip it. Choice: `git commit --no-verify` for every commit on this branch. TS reference: `.husky/pre-commit`. Files: none.
