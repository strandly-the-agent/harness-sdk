Track `h12fa`: fix Phase 12 correctness findings in `create_harness` and the portable config. Read `.rust-port/briefs/run5-fix-common.md` first and follow it.

Worktree `/mnt/workspace/wt-h12fa` (branch `wip/h12fa`), `CARGO_TARGET_DIR=/tmp/ct-h12fa`, staging dir `.rust-port/staging/h12fa/`, decisions P-1120 to P-1129.

Report to read: `.rust-port/reviews/phase12-review-correctness.md`. Your findings:
- `correctness-1`: `agent.rs` `session_storage` (a session `dir` with no final component, and a last component containing a backslash).
- `correctness-2`: `config/options.rs` `apply_agent_config` (portable `agentConfig` keys dropped or rejected: `traceAttributes`, `toolExecutor`, `sandbox: false`, `retryStrategy: null`, the object form of `backgroundTasks`, and the harness-level keys `instructions`, `caching`, `builtinTools`, `builtinPlugins`, `mcpServers`, `interventions` that TS honours when the top-level key is at its default). Follow TS `harness-ts/src/config.ts:370-393` and `agent.ts:364,512` for precedence. A key that has no Rust form must be an error or a logged, documented drop per PROMPT.md section 11 item 37 and decision P-938 (read it: `grep -n '^\*\*P-938\.' .rust-port/DECISIONS-pending.md`); correct the `DIVERGENCES.md` row (stage the replacement row).
- `correctness-4`: secrets in `Debug` output (`config.rs` `HarnessConfigMcpServers` prints MCP `env` and `headers` values; `agent.rs` `McpServersOption` prints `auth.clientSecret`). Redact every secret-bearing value reachable from the `Debug` of `HarnessAgentOptions` and `HarnessAgentConfig`; test with a value that must not appear in `format!("{:?}")` and `{:#?}`.
- `correctness-7`: `config/normalize.rs` `subagent.maxDepth` above `u32::MAX`.

You own `harness-rs/strands-harness/src/agent.rs`, `src/config.rs`, `src/config/`, and their test files (`src/agent/tests.rs`, `src/agent/tests/*.rs`, `src/config/tests.rs`). Other tracks: `h12fb` owns `src/telemetry.rs`, `src/lib.rs` and the SDK `agent/run.rs`; `h12fc` owns `src/builtin_tools.rs`, `src/plugins/` and the SDK `mcp/` and `background_tasks/`.
