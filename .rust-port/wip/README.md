# Work in progress from unfinished tracks

Each folder holds `git format-patch` output from a worktree branch that wasn't integrated when the session ended. Apply on a scratch branch from the named base (`git switch -c wip/<track> <base> && git am .rust-port/wip/<track>/*.patch`), then cherry-pick onto `feat/rust-sdk`, fold `.rust-port/staging/<track>/` into the shared logs, and delete the folder here.

- `phase6c/`: Phase 6 item 3 (`ContextManager`), base `867c82366`, brief `.rust-port/briefs/phase6-context-manager.md`; unfinished (gates green at its tip; left to do: the 55 `summarize-strategy.test.ts` cases, 11 of the 18 session stash cases, passing `agent_metadata` to `summarize` (waits on P-304), and removing the now-stale `allow(dead_code)` on the conversation-manager helpers).

If `/mnt/workspace/wt-phase6c` still exists, its branch may hold newer commits than these patches; prefer them.
