# Work in progress from unfinished tracks

Each folder holds `git format-patch` output from a worktree branch that wasn't integrated when the session ended. Apply on a scratch branch from the named base (`git switch -c wip/<track> <base> && git am .rust-port/wip/<track>/*.patch`), then cherry-pick onto `feat/rust-sdk`, fold `.rust-port/staging/<track>/` into the shared logs, and delete the folder here.

- `phase9fix/`: Phase 9 review fixes, base `7cd6f6ab4`, brief `.rust-port/briefs/phase9-fix.md`; unfinished (check the staged `dispositions.md` for what's left).
- `phase6c/`: Phase 6 item 3 (`ContextManager`), base `867c82366`, brief `.rust-port/briefs/phase6-context-manager.md`; unfinished (strategies, presets, stash and `retrieve_context` so far).

If `/mnt/workspace/wt-phase9fix` or `/mnt/workspace/wt-phase6c` still exist, their branches may hold newer commits than these patches; prefer them.
