# Work in progress from unfinished tracks

Each folder holds `git format-patch` output from a worktree branch that wasn't integrated when the session ended. Apply on a scratch branch from the named base (`git switch -c wip/<track> <base> && git am .rust-port/wip/<track>/*.patch`), then cherry-pick onto `feat/rust-sdk` (`.rust-port/tools/cpauto.sh <track>`), fold `.rust-port/staging/<track>/` into the shared logs (`.rust-port/tools/fold.py <track>`), and delete the folder here.


If `/mnt/workspace/wt-<track>` still exists, its branch may hold newer commits than these patches; prefer them.
