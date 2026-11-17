# Lead tooling for the run (internal)

Copy these to `/mnt/workspace/` at the start of a session (the scripts refer to each other and to the checkout by absolute path, `/mnt/workspace/harness-sdk`).

- `setup-env.sh`: rebuilds the environment on an empty sandbox (Rust stable and 1.96.0 under `/mnt/workspace/.rust`, the TS spec export, the donor clone, the `/tmp` symlinks, Node, `npm ci`, `npm run build`, cargo-deny). Run it in the background and read `/mnt/workspace/setup-env.log`. After `npm ci`, run `git config core.hooksPath /dev/null` in the checkout and in every worktree so cherry-picks don't run the husky hook.
- `cp.sh <track>`: cherry-picks `merge-base..wip/<track>` onto the current branch and lists conflicted files. `cpauto.sh [<track>]`: the same, resolving `test-title-map.json` conflicts with `jsonmerge.py` and stopping at any other conflict; without an argument it continues a cherry-pick after you staged a resolution. `cont.sh`: `cherry-pick --continue` without the editor.
- `union.py <file>...`: resolves conflict hunks by keeping both sides (imports, module lists). `res.py <file> <n>`: replaces the n-th conflict hunk with stdin.
- `jsonmerge.py`: 3-way merge of `strands-rs/docs/test-title-map.json` per spec and title during a cherry-pick conflict.
- `fold.py <track>`: folds `.rust-port/staging/<track>/` into the shared logs (run from the repo root): appends decisions and TS bugs, adds or replaces divergence rows by first column, replaces PARITY rows by first column. Then `git rm -r .rust-port/staging/<track>`.
- `hfold.py <track>`: the same as `fold.py` for a harness track (`harness-rs/docs/`). `integ.sh <track>`: after `cpauto.sh <track>`, folds a fix track's staging dir, keeps its `dispositions.md` and `handoff.md` under `.rust-port/handoff/`, remaps commit hashes and commits.
- `remap.py <file>...`: rewrites short commit hashes that are not ancestors of HEAD to the HEAD commit with the same subject (after cherry-picks). `ours.py ours|theirs <file>...`: resolves every conflict hunk by keeping one side.
- `assemble12.py`: builds `harness-rs/docs/reviews/phase-12.md` from `.rust-port/handoff/phase12a-dispositions.md` and `phase12-dispositions-*.md`.

