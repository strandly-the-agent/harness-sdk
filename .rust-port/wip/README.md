# Work in progress from unfinished tracks

Each folder holds `git format-patch` output from a worktree branch that wasn't integrated when the session ended. Apply on a scratch branch from the named base (`git switch -c wip/<track> <base> && git am .rust-port/wip/<track>/*.patch`), then cherry-pick onto `feat/rust-sdk` (`.rust-port/tools/cpauto.sh <track>`), fold `.rust-port/staging/<track>/` into the shared logs (`.rust-port/tools/fold.py <track>`), and delete the folder here.

- `p10fixa/`: Phase 10 review fixes (span lifecycle and span output), base `e7088f7d7`, 15 commits, brief `.rust-port/briefs/run3-p10fixa.md`. The track reported complete: its standard gate was green in its worktree (4387 passed); `gates.sh full` was not run. Open after it: parity-9 (11 of the 47 `agent.tracer` cases are not ported: three structured-output cases, five baggage cases, the post-middleware model span, concurrent tool spans, the error after the cycle closed), parity-11 (tests that read `agent_metrics.json`, `agent_traces.json` and `metric_points.json`), correctness-7 (trace attributes on background tool runs; the fix crosses `types/agent.rs` and `background_tasks/`, recommended fix in its dispositions).
- `p10fixb/`: Phase 10 review fixes (the public `otel` surface and setup), base `e7088f7d7`, 17 commits, brief `.rust-port/briefs/run3-p10fixb.md`. The track reported complete: its standard gate plus deny, `feat_otel` and MSRV were green in its worktree (4367 passed). Both tracks edited `strands-rs/docs/test-title-map.json`; `cpauto.sh` merges it.

Neither track is integrated: after applying both, run `cargo fmt --all` and `gates.sh full`, then write `strands-rs/docs/reviews/phase-10.md` from the two `dispositions.md` files.

If `/mnt/workspace/wt-<track>` still exists, its branch may hold newer commits than these patches; prefer them.
