# Run state

## Where we are

- Current phase: Phase 3 (agent loop, hooks and tools). Phases 0-2 closed: Phase 0 at `0371b1577`, Phase 1 at `6b89c5dd6`, Phase 2 at `0724c715f` (review files `strands-rs/docs/reviews/phase-{0,1,2}.md`; full gates pass, 749 tests). Phase 3 done: tools layer and macros (`0d9a8e91a`..`0724c715f`, seams for the loop listed in P-71..P-80). In progress: hook registry (`briefs/phase3-hooks.md`). Next: the agent loop brief (Agent, AgentConfig, builder, invoke/stream, stream records, executors, loop semantics, direct calls, as_tool, printer, retry wiring, HookRecorder/test_agent), then the Phase 3 review gate.
- Phase base commits: Phase 0 base = `7296caba7`; Phase 1 base = `1a04bf10a`; Phase 2 base = `5383d5c20` (retry landed at `fd48e74f0`; models from `836087c6a`); Phase 3 base = `913a2f3bb` (Phase 3 commits start at `0d9a8e91a`) (Phase 1 commits start at `7a8080e54`; Phase 0 files committed later are docs, conformance and `.rust-port/` only)

## Credentials (checked without printing)

- AWS: ok (default chain), `AWS_REGION=us-west-2`
- `ANTHROPIC_API_KEY`: missing
- `OPENAI_API_KEY`: missing

## Environment notes

- The sandbox is ephemeral and each session lives at most about 8 hours. Work persists only through the branch: after each commit batch, push `feat/rust-sdk` to `https://github.com/strandly-the-agent/harness-sdk.git` (remote name `fork`). Never push to `strands-agents/harness-sdk` (P-2).
- Repo clone: `/mnt/workspace/harness-sdk`. Node 22.23.4 (from `.node-version`) unpacked at `/mnt/workspace/.node`; put `/mnt/workspace/.node/bin` first on `PATH`.
- Rust: rustup stable (1.100.0 at setup) plus `1.96.0` (MSRV, minimal profile); `cargo-deny` 0.20.2 installed from the release binary into `~/.cargo/bin`.
- Host: Linux aarch64, 4 CPUs. Keep `cargo` parallelism in mind when running subagents side by side (use separate `CARGO_TARGET_DIR`s for reviewers).
- `https://github.com/TheWitcherish/strands-harness-rs.git` returned "Repository not found" at setup; the reference-only code is unavailable, so the run proceeds without it.
- Pushing commits that touch `.github/workflows/` is refused (no `workflow` scope); CI files are staged in `.rust-port/ci/` (P-12).
- `cargo` is in `~/.cargo/bin`; run `source ~/.cargo/env` first in every shell.
- Restore on a fresh sandbox: clone the fork, `git checkout feat/rust-sdk`, add `origin` = strands-agents/harness-sdk, re-create `/tmp/ts-spec` (`git archive 7296caba7 strands-ts harness-ts shared AGENTS.md`), clone the donor to `/tmp/donor-strandly` at `06ae0d6db`, install the toolchains above, `npm ci && npm run build` with Node 22.23.4.

## Open threads

- The sandbox has dropped connections five times, under load and while idle. It recovers by itself; commit and push after every finished item. Run at most 2 subagents at once and use `CARGO_BUILD_JOBS=2`.
- The scheduled 9-hour resume job could not be created from this run (the schedule tool needs a verified team requester from Slack, the portal or a GitHub mention).
- Briefs ready: `briefs/phase1-roundtrip.md`, `briefs/phase2-models.md`, `briefs/phase2-retry.md`.
