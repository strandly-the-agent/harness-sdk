# State

## Where we are

- Status: in progress (fresh start on 2026-11-15)
- Current phase: Phase 2 review gate; Phase 3 core implementation in parallel
- Current item: Phase 3 core item 4 (executors). Items 1-3 done (28c89da98 hooks + agent views, 19bb0b2ef tools); open: executors, `Agent`/config/stream events, loop semantics (retry wiring, `ModelProxy`, P-44 `summarize` routing), direct tool calls/`as_tool`/printer, `test_agent`. Remove the temporary `allow(dead_code)` in `types/agent.rs`, `hooks/events.rs`, `hooks/registry.rs`, `tools/tool.rs`, `tools/mod.rs` once the loop uses them (P-50)
- Phase base commits: Phase 0 = 7296caba7; Phase 1 = d2cce53e2 (Phase 0 review diff 7296caba7..d2cce53e2; Phase 0 fixes landed in 95c800b0e..0e4b20905); Phase 2 = 0e4b20905 (Phase 1 review diff d2cce53e2..0e4b20905); Phase 3 = ef33e6992 (Phase 2 review diff 0e4b20905..ef33e6992)

## Credentials

- ANTHROPIC_API_KEY: missing
- OPENAI_API_KEY: missing
- AWS: ok (default chain, region set in env)

## Environment notes

- The sandbox is ephemeral between sessions; only `/mnt/workspace` survives a resume and a fresh session may start empty. The branch is the system of record.
- The branch is persisted by pushing `feat/rust-sdk` to the remote `fork` (`https://github.com/strandly-the-agent/harness-sdk.git`) as `feat/rust-sdk-run-20261115`. Never force-push; never push anywhere else. Older branches on that remote (`feat/rust-sdk`, `feat/rust-sdk-fresh`, `feat/rust-sdk-prev-20261115`) are unrelated earlier attempts and are not used.
- Rust toolchains live in `/mnt/workspace/.rust` (`RUSTUP_HOME=/mnt/workspace/.rust/rustup`, `CARGO_HOME=/mnt/workspace/.rust/cargo`, add `$CARGO_HOME/bin` to `PATH`). Stable is 1.100.0; MSRV toolchain is 1.96.0. cargo-deny 0.20.2 is installed there. Node 22.23.4 is in `/mnt/workspace/.node/bin` (`npm ci` and `npm run build` done at the repo root; `node_modules/` and `strands-ts/dist/` are untracked build output).
- `/tmp` does not survive a restart, so the spec export, donor and reference clones live under `/mnt/workspace/` and are symlinked: `/tmp/ts-spec -> /mnt/workspace/ts-spec`, `/tmp/donor-strandly -> /mnt/workspace/donor-strandly`, `/tmp/ref-witcherish -> /mnt/workspace/ref-witcherish`. Recreate the symlinks (and the clones, per PROMPT.md section 9.1) if missing.
- Host: Linux aarch64, 4 cores.

## Open threads

- CI workflows can't be pushed under `.github/workflows/` with this environment's token; they are staged in `.rust-port/ci-workflows/` (P-13). Any later CI edits go there too.

- The reference-only repository `https://github.com/TheWitcherish/strands-harness-rs.git` returns "repository not found" (2026-11-15). It is reference-only (decision #4), so work continues without it.
- Unused-dependency cargo warnings (cargo 1.100 `unused_dependencies`) appear while crates are placeholders; they go away as modules use the dependencies.
