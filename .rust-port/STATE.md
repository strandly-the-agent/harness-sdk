# Run state

## Where we are

- Current phase: Phase 2 (model abstraction, streaming and retry), review gate in progress. Phase 0 closed at `0371b1577` (`strands-rs/docs/reviews/phase-0.md`); Phase 1 closed at `6b89c5dd6` (`phase-1.md`; full gates pass, 573 tests). Phase 2 items done: models, Model trait, retry, MockModel (HookRecorder and test_agent are Phase 3 per the spec). Phase 2 correctness findings are being fixed (`briefs/phase2-fixes.md`); API and parity reviews pending.
- Phase base commits: Phase 0 base = `7296caba7`; Phase 1 base = `1a04bf10a`; Phase 2 base = `5383d5c20` (retry landed at `fd48e74f0`; models from `836087c6a`); Phase 3 base = TBD (Phase 1 commits start at `7a8080e54`; Phase 0 files committed later are docs, conformance and `.rust-port/` only)

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
