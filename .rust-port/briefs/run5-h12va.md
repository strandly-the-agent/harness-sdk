Track `h12va`: fix the harness-side findings of the Phase 12 verification pass. Read `.rust-port/briefs/run5-fix-common.md` first and follow it.

Worktree `/mnt/workspace/wt-h12va` (branch `wip/h12va`), `CARGO_TARGET_DIR=/tmp/ct-h12va`, staging dir `.rust-port/staging/h12va/`, decisions P-1210 to P-1219.

Report to read: `.rust-port/reviews/phase12b-review-correctness.md` (probes in `.rust-port/reviews/phase12b-review-correctness-probe/`). Your findings:
- `correctness-b-1` (the API reviewer's `api-b-5` in `.rust-port/reviews/phase12b-review-api-parity.md` is the same defect; one fix, two disposition rows): `shutdown_telemetry()` returns `AlreadyShutdown` on its second call when a `create_harness` ran in between. Keep a shut-down state so setup and later shutdowns after it are no-ops that return `Ok(())`, test create, shutdown, create, shutdown, and say in the rustdoc that a harness created after the call is not traced.
- `correctness-b-2`: `tools/web_search.rs` `one_line` collapses white space with Rust's set where TS uses `/\s+/g` and `.trim()`; share one JS white-space predicate with `text::js_trim`.
- `correctness-b-5`: `config/options.rs`: `agentConfig.caching: "auto"` must be the explicit request TS makes it (`CachingOption::On`), with a test against a model without caching.
- `correctness-b-6`, harness half: `config.rs` `redact_mcp_servers` still prints a server's `url` whole (userinfo and query string) and its `args`. Redact the userinfo and the query string of `url` and the `args` values; extend the existing `Debug` tests. The SDK half (`strands-rs/strands/src/mcp/config.rs`) belongs to track `h12vb`: use the same visible shape (agree by reading its brief, `.rust-port/briefs/run5-h12vb.md`: the URL keeps scheme, host, port and path; `args` prints as a count).

You own `harness-rs/strands-harness/src/telemetry.rs`, `src/telemetry/`, `src/tools/web_search*`, `src/text.rs`, `src/config.rs`, `src/config/` with their tests. Track `h12vb` works in the SDK at the same time; the lead edits `harness-rs/docs/`, `harness-rs/AGENTS.md`, rustdoc text in `src/lib.rs` and `src/tools/subagent.rs`, and `tests/public_api.rs`.
