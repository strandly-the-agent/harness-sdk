# Work in progress from unfinished tracks

Each folder holds `git format-patch` output from a worktree branch that wasn't integrated when the session ended. Apply on a scratch branch from the named base (`git switch -c wip/<track> <base> && git am .rust-port/wip/<track>/*.patch`), then cherry-pick onto `feat/rust-sdk`, fold `.rust-port/staging/<track>/` into the shared logs, and delete the folder here.

- `phase6c/`: Phase 6 item 3 (`ContextManager`), base `867c82366`, brief `.rust-port/briefs/phase6-context-manager.md`; unfinished (strategies, presets, stash, `retrieve_context` and the agentic tools so far; its implementer was still running when the session ended).

If `/mnt/workspace/wt-phase6c` still exists, its branch may hold newer commits than these patches; prefer them.
