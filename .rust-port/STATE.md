# State

- Current phase: 1 (types, wire and errors). Current item: Phase 1 review gate and fixes (all other Phase 1 items ticked). Resume from the first unticked item in `PLAN-TRACKER.md`.
- Phase 1 implementation: `502e6101b`..HEAD. `gates.sh full` green (262 all-features tests), each provider feature alone passes clippy. Fixtures not yet round-tripped (stream, hooks, manifests, stash, telemetry) are listed with their owning subsystem in `strands-rs/docs/PARITY.md`. Logged P-20..P-31.
- Phase 0 base commit: `7296caba7`. Phase 0 closed: review gate fixed in `13a6a3155`..`7f48a8bd7`, dispositions in `strands-rs/docs/reviews/phase-0.md`, `gates.sh full` and per-feature clippy green.
- Phase 1 base commit: `7f48a8bd7` (the last code/docs commit of Phase 0; the commit after it only updates this run folder).
- Credentials: aws=ok (Bedrock invoke), anthropic=missing, openai=missing. Anthropic and OpenAI live tests will record the missing-credential error in `live.md`.
- Toolchains: stable 1.99.1 (rustfmt, clippy), 1.96.0 (minimal). cargo-deny 0.20.2 (prebuilt binary). Node v22.14.0 present (`.node-version` asks 22.23.4).
- Environment: 2 CPUs, 8 GB RAM, about 5.5 GB free disk after toolchains. The sandbox filesystem does not persist across sessions, so the branch is pushed to `strandly-the-agent/harness-sdk` (`feat/rust-sdk` only) as the way it is returned (P-2).
- Inputs: `/tmp/ts-spec` exported from the pin; donor at `/tmp/donor-strandly` (git worktree of `06ae0d6db`); TheWitcherish reference repo returned "Repository not found" on clone, so it is unavailable for this run.
- Disk: a clean `gates.sh full` run fills `~/target` to about 4 GB (dependency debug info is off, P-9); about 4.7 GB is free with `~/target` empty and no `node_modules`. Run `npm ci`/`npm run build` only with `~/target` cleared, and delete `node_modules` and `strands-ts/dist` afterwards.
- Tools: actionlint 1.7.12 binary at `/tmp/actionlint` (re-download if the sandbox recycles); run with `-ignore 'label ".*" is unknown'` for the custom runner labels.
- Open threads: the fork token cannot push `.github/workflows/` changes (no `workflow` scope). The Rust workflows and the `ci.yml` edit are held in `.rust-port/workflows/` (P-16) and need moving by someone with that scope. The CI workflows have not run on GitHub.
- Phase 0 review gate notes: fixture regeneration needs `npm ci && npm run build` with `~/target` cleared (about 0.6 GB for node_modules and dist); delete them afterwards. Integration-test crate roots (`tests/main.rs`) must carry the crate-level clippy allow listed in `strands-rs/AGENTS.md`; `indexing_slicing` is denied in library code.
