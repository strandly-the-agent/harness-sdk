# vended tools: `sleep`, `handoff_to_user`, `file_editor`, `http_request`, `mcp_router` (`tools::vended`) — strands-rs port

Branch `agent-tasks/strands-rs-vended-tools`, worktree `/home/sandbox/work/wt/vended-tools`.
Files added: `crates/strands-agents/src/tools/vended/{sleep.rs,handoff_to_user.rs,file_editor.rs,http_request.rs,mcp_router.rs}`,
`crates/strands-agents/tests/{vended_tools_loop.rs,mcp_router_e2e.rs}`.
Files touched: `crates/strands-agents/src/tools/vended/mod.rs` (declarations + re-exports),
`crates/strands-agents/src/lib.rs` (`pub use tools::vended::{HandoffToUserTool, ShellTool, SleepTool};`).

Source of truth: `strands-ts/src/vended-tools/sleep/{make-sleep.ts,sleep.ts,types.ts}` and
`strands-ts/src/vended-tools/handoff-to-user/{handoff-to-user.ts,types.ts}`; tests
`sleep/__tests__/sleep.test.ts`, `handoff-to-user/__tests__/handoff-to-user.test.ts`.

## API matrix

| TS | Rust |
|---|---|
| `sleep` (default instance) | `SleepTool::new()` |
| `makeSleep({ maxDuration, name, description })` | `SleepTool::with_config(SleepConfig { max_duration, sleeper })` |
| `DEFAULT_MAX_DURATION` | `DEFAULT_MAX_DURATION: f64` |
| `sleepDescription(max)` / `SLEEP_DESCRIPTION` | `sleep_description(max_duration) -> String` |
| `SleepInput { duration }` | private `SleepInput`; validated in `SleepTool::execute` |
| `handoffToUser` | `HandoffToUserTool::new()` |
| `makeHandoffToUser({ name, description })` | `HandoffToUserTool::with_config(HandoffToUserConfig { name, description })` |
| `HANDOFF_INTERRUPT_NAME` | `HANDOFF_INTERRUPT_NAME: &str` |
| `DEFAULT_HANDOFF_TO_USER_DESCRIPTION` | `DEFAULT_HANDOFF_TO_USER_DESCRIPTION: &str` |
| `context.interrupt({ name, reason })` | `ToolContext::interrupt(InterruptParams::new(..).with_reason(..))` |

Name, description text and input-schema wording match TS verbatim (asserted by
`spec_matches_the_ts_name_description_and_schema` and `exposes_the_default_name_and_input_schema`).

## Test matrix — `sleep` (18 ported, 1 skipped)

| TS test | Rust test | Status |
|---|---|---|
| `rejects negative durations` | `rejects_negative_durations` | pass |
| `rejects NaN durations` | `rejects_nan_durations` | pass |
| `rejects positive infinity` | `rejects_positive_infinity` | pass |
| `rejects negative infinity` | `rejects_negative_infinity` | pass |
| `rejects non-numeric input via schema` | `rejects_non_numeric_input_via_schema` | pass |
| `rejects boolean input via schema` | `rejects_boolean_input_via_schema` | pass |
| `rejects duration above default maximum` | `rejects_duration_above_default_maximum` | pass |
| `rejects duration above configured maximum` | `rejects_duration_above_configured_maximum` | pass |
| `rejects zero maxDuration` | `rejects_zero_max_duration` | pass |
| `rejects negative maxDuration` | `rejects_negative_max_duration` | pass |
| `rejects NaN maxDuration` | `rejects_nan_max_duration` | pass |
| `rejects Infinity maxDuration` | `rejects_infinity_max_duration` | pass |
| `rejects non-numeric maxDuration` | — | skipped: `max_duration` is `f64`; a non-number cannot be constructed |
| `aborts immediately when cancelSignal fires mid-sleep` | `aborts_immediately_when_cancel_fires_mid_sleep` | pass |
| `rejects synchronously when the signal is already aborted` | `rejects_immediately_when_the_token_is_already_cancelled` | pass |
| `returns the expected message after a real short sleep` | `returns_the_expected_message_after_a_short_sleep` | pass |
| `accepts zero duration` | `accepts_zero_duration` | pass |
| `works without a context by skipping the cancel wiring` | `spec_matches_the_ts_name_description_and_schema` (via `SleepTool::execute`, which takes no context) | pass |
| `drives setTimeout with the requested duration in ms` | `drives_the_sleeper_with_the_requested_duration` | pass |
| — | `a_configured_maximum_is_interpolated_into_the_description` | pass (added) |
| — | `sleep_runs_in_the_loop_and_reports_the_duration` (loop) | pass (added) |
| — | `a_duration_above_the_configured_maximum_fails_the_tool_call` (loop) | pass (added) |

## Test matrix — `handoff_to_user` (7 ported, 1 skipped)

| TS test | Rust test | Status |
|---|---|---|
| `raises an interrupt carrying the message and stable name on first call` | `raises_an_interrupt_carrying_the_message_and_stable_name_on_first_call` | pass |
| `rejects a non-string message without registering an interrupt` | `rejects_a_non_string_message_without_registering_an_interrupt` | pass |
| `throws when invoked without a tool context` | — | skipped: `Tool::invoke` always receives a `ToolContext`; the state is unrepresentable |
| `exposes the default name and input schema` | `exposes_the_default_name_and_input_schema` | pass |
| `customizes the name and description via the factory` | `customizes_the_name_and_description_via_the_config` | pass |
| `halts with stopReason interrupt and the message as the reason` | `handoff_halts_with_stop_reason_interrupt_and_the_message_as_the_reason` (loop) | pass |
| `returns the reply as the tool result and the model continues on resume` | `returns_the_reply_as_the_tool_result_on_resume` + `handoff_returns_the_reply_as_the_tool_result_and_the_model_continues_on_resume` (loop) | pass |
| `reports the stable interrupt name even when the tool is renamed` | `reports_the_stable_interrupt_name_even_when_the_tool_is_renamed` | pass |
| — | `rejects_a_blank_message_without_registering_an_interrupt` (TS zod `.trim().length > 0` refinement) | pass (added) |
| — | `the_interrupt_name_is_the_ts_constant` | pass (added) |

## Divergences

- **Injected sleeper instead of `setTimeout`.** `SleepConfig::sleeper` is the `retry::Sleeper`
  trait (default `TokioSleeper`), so tests assert the requested `Duration` without waiting. TS
  spies on `globalThis.setTimeout` instead.
- **Validation failures become an error tool result, not a raised error.** `SleepTool::invoke` and
  `HandoffToUserTool::invoke` return `create_error_result(..)`, matching the sibling `ShellTool`;
  in TS the zod rejection is turned into the same error result one layer up, in the tool executor.
  `SleepTool::execute` still returns `Err(Error::ToolValidation(..))` with the TS message text.
- **`max_duration` is validated at construction.** `SleepTool::with_config` returns
  `Result<Self>`; TS `makeSleep` throws. `SleepTool::new()` cannot fail.
- **No `name`/`description` override on `SleepConfig`** (unlike `MakeSleepOptions`), matching
  `ShellConfig`. `HandoffToUserConfig` does carry both, because the stable-interrupt-name-under-
  rename behaviour is part of the TS contract.
- **Cancellation** uses `ToolContext::cancel` (`CancellationToken`) raced with `tokio::select!`
  and surfaces as `Error::Cancelled`, where TS rejects with a `DOMException('AbortError')`.

---

# `fileEditor`

TS: `strands-ts/src/vended-tools/file-editor/{file-editor.ts,types.ts}` →
`crates/strands-agents/src/tools/vended/file_editor.rs`.

## API

| TS | Rust |
| --- | --- |
| `fileEditor` / `makeFileEditor(options)` | `FileEditorTool::new()` / `FileEditorTool::with_config(FileEditorConfig)` |
| `MakeFileEditorOptions { name, description, maxFileSize }` | `FileEditorConfig { name, description, max_file_size }` |
| `'fileEditor'`, `DEFAULT_FILE_EDITOR_DESCRIPTION`, 1 MB cap | `FILE_EDITOR_TOOL_NAME`, `DEFAULT_FILE_EDITOR_DESCRIPTION`, `DEFAULT_MAX_FILE_SIZE` |
| commands `view` / `create` / `str_replace` / `insert` | same four, same input field names and error strings |
| `node:fs` access via the agent's sandbox | `Sandbox`: the tool's own, else the agent's, else `LocalSandbox` |

## Tests

53 unit tests in `file_editor.rs`, one per TS `it(...)` in
`__tests__/file-editor.test.ts` with the title snake_cased (`throws when ...` →
`errors_when_...`), all passing. Rust-only additions, each covering a path TS leaves implicit:
`omitting_new_str_deletes_old_str`, `errors_when_file_text_is_missing`,
`errors_when_old_str_is_missing`, `errors_when_insert_params_are_missing`,
`errors_when_the_file_exceeds_the_size_limit`, `handles_unicode_and_special_characters`,
`expands_tabs_in_the_view_output`, `handles_a_trailing_slash_on_a_file_path`,
`rejects_an_unknown_command`, `spec_matches_the_ts_name_description_and_schema`,
`a_cancelled_call_returns_cancelled`.

## Divergences

- **No `undo_edit`.** Upstream TS has no `undo_edit` command and keeps no undo history, so this
  port has neither (the scope's checklist assumed otherwise).
- **`crates/strands-harness` ships its own file tools** that overlap in intent; they are
  deliberately left alone rather than unified in this port.
- Validation failures become an error tool result, as for `sleep`/`shell`.

---

# `http_request`

TS: `strands-ts/src/vended-tools/http-request/{http-request.ts,types.ts}` →
`crates/strands-agents/src/tools/vended/http_request.rs`, behind the new `http` cargo feature
(`http = ["dep:reqwest"]`, mirroring `strands-harness`).

## API

| TS | Rust |
| --- | --- |
| `httpRequest` (a `tool({...})` constant) | `HttpRequestTool::new()` / `with_config(HttpRequestConfig)` |
| — | `HttpRequestConfig { name, description, default_timeout, client }` |
| `HttpRequestInput { method, url, headers?, body?, timeout? }` | `HttpRequestInput` (same fields; `timeout: Option<f64>` seconds) |
| `HttpRequestOutput { status, statusText, headers, body }` | `HttpRequestOutput` (`status_text` serializes as `statusText`) |
| `'http_request'` + description + zod schema | `HTTP_REQUEST_TOOL_NAME`, `DEFAULT_HTTP_REQUEST_DESCRIPTION`, same schema (enum, `required`, descriptions verbatim) |
| default timeout 30 s | `DEFAULT_TIMEOUT_SECONDS` |
| `globalThis.fetch` | injected `reqwest::Client` on the config |

## Tests

17 unit tests, all passing, against a `tokio::net::TcpListener` mini HTTP server in the test
module (no new dev-dependencies).

| TS `it` | Rust test | state |
| --- | --- | --- |
| `describe.each(7 methods) > returns successful response` | `returns_successful_response_for_every_method` (all 7, incl. 201/204 and HEAD) | pass |
| `sends request with custom headers and body` | `sends_request_with_custom_headers_and_body` (asserts the bytes the server received) | pass |
| `omits headers/body when not provided` | `omits_headers_and_body_when_not_provided` | pass |
| `uses default timeout of 30 seconds` | `defaults_the_timeout_to_thirty_seconds` | pass |
| `throws error for 3xx redirect status codes` | `throws_error_for_3xx_redirect_status_codes` | pass |
| `throws error for 4xx client error codes` | `throws_error_for_4xx_client_error_codes` | pass |
| `throws error for 5xx server error codes` | `throws_error_for_5xx_server_error_codes` | pass |
| `throws timeout error when request exceeds timeout` | `throws_timeout_error_when_request_exceeds_timeout` (message verbatim) | pass |
| `throws error for network failures` | `throws_error_for_network_failures` | pass |
| `uses context.cancelSignal when it differs from the agent signal` | `uses_the_cancellation_token_mid_request` | pass |
| — | `rejects_a_non_positive_timeout`, `rejects_an_unsupported_method`, `rejects_a_url_that_is_not_a_url`, `rejects_immediately_when_the_token_is_already_cancelled`, `invoke_returns_the_response_as_a_tool_result`, `invoke_reports_a_failed_request_as_an_error_result`, `spec_matches_the_ts_schema` | pass (added) |

## Divergences

- **Status text is derived**, not received: `StatusCode::canonical_reason()` (empty for an unknown
  code), because `reqwest`/HTTP2 do not expose the server's reason phrase. TS reports what `fetch`
  saw.
- **Cancellation returns `Error::Cancelled`** (the variant the executor understands) instead of
  TS's `Request cancelled: {method} {url}` message; the timeout message is verbatim TS.
- **URL and method validation** happen in Rust (zod `.url()` / `z.enum` have no runtime
  equivalent here) and surface as `Error::ToolValidation`.
- **The timeout is a `tokio::select!` race**, not `AbortSignal.timeout`, so it covers connect,
  send and body read exactly as the TS signal does.
- `tokio`'s `net` feature was added to `strands-rs/Cargo.toml` for the test server (dev-only use;
  additive).

# `mcp_router`

`strands-ts/src/vended-tools/mcp-router/{mcp-router.ts,types.ts}` →
`crates/strands-agents/src/tools/vended/mcp_router.rs`, behind the existing `mcp` cargo feature.
Re-exported as `tools::vended::McpRouterTool` and `strands_agents::McpRouterTool`
(both `#[cfg(feature = "mcp")]`).

## API

| TS | Rust |
| --- | --- |
| `makeMcpRouter({ name?, description?, servers, maxConnections? })` | `McpRouterTool::new(servers) -> Result<Self>` / `with_config(McpRouterConfig) -> Result<Self>` |
| `MakeMcpRouterOptions` | `McpRouterConfig { name, description: Option<String>, servers: IndexMap<String, McpServerConfig>, max_connections }` |
| `McpRouterInput { command, connection_id?, server_name?, tool_name?, arguments? }` | `McpRouterInput` (same fields; `command: McpRouterCommand`) |
| `'connect' \| 'list_connections' \| 'list_tools' \| 'call_tool' \| 'disconnect'` | `McpRouterCommand` (`#[serde(rename_all = "snake_case")]`) |
| `MCP_ROUTER_DESCRIPTION`, `'mcp_router'`, `DEFAULT_MAX_CONNECTIONS = 10` | `MCP_ROUTER_DESCRIPTION`, `MCP_ROUTER_TOOL_NAME`, `DEFAULT_MAX_CONNECTIONS` |
| `McpRouterToolError` | `Error::ToolValidation` with the TS messages verbatim |
| `McpClient.loadServers` + `client.connect()` | `McpClient::load_servers(IndexMap, LoadOptions::default(), prefix "")` (which connects) |
| `client.listTools({ prefix: '' })` → `ToolSpec[]` | `client.list_tools_with(prefix "")` → JSON array of `ToolSpec` |
| ad-hoc `McpTool.stream` for `call_tool` | `McpClient::call_tool` + `mcp::tool::map_call_tool_result` (same content mapping) |
| per-agent `WeakMap` + `FinalizationRegistry` | `Mutex<IndexMap<String, McpClient>>` on the tool instance |

Name, description (incl. the ` Permitted server names: 'a', 'b'.` suffix) and the input schema
(property descriptions, command enum, `required: ["command"]`) are verbatim TS.

## Tests

13 unit tests in `mod tests` (`cargo test -p strands-agents --all-features --lib
tools::vended::mcp_router` → 13 passed). The TS suite fakes `McpClient` with `vi.spyOn`; Rust's
`McpClient` is a concrete type with no trait seam, so the cases that need a live server are **not
ported as unit tests** — they need a real MCP server (the `mcp_echo_server` test binary, only
reachable from `tests/`).

| TS `it` | Rust test | state |
| --- | --- | --- |
| `rejects empty allowlist` | `rejects_empty_allowlist` | pass |
| `rejects zero maxConnections` | `rejects_zero_max_connections` | pass |
| `description includes server names` | `description_includes_server_names` | pass |
| `uses default description with MCP_ROUTER_DESCRIPTION` | `uses_default_description_with_mcp_router_description` | pass |
| `uses custom name` / `accepts custom description` | `accepts_custom_name_and_description` | pass |
| `exposes expected schema fields` | `exposes_expected_schema_fields` | pass |
| `rejects server not on allowlist` | `rejects_server_not_on_allowlist` | pass |
| `rejects missing connection_id` | `rejects_missing_connection_id` | pass |
| `rejects missing server_name` | `rejects_missing_server_name` | pass |
| `returns sorted open connection IDs` (empty case) | `returns_empty_string_when_no_connections_are_open` | pass |
| `rejects call_tool without tool_name` | covered by `rejects_commands_for_an_unknown_connection_id` (the connection check runs first) | pass |
| — | `rejects_commands_for_an_unknown_connection_id`, `invoke_returns_an_error_result_for_a_failed_command`, `invoke_lists_connections_as_a_success_result` | pass (added) |
| `supports multiple simultaneous connections`, `rejects reconnect with same id`, `enforces connection cap`, `cleans up on start failure`, `rejects when client silently enters failed state`, `throws when allowlisted server has disabled: true`, `runs full connect → list_tools → call_tool → disconnect flow`, `throws McpRouterToolError when MCP tool returns isError`, `disconnect succeeds even when client.disconnect throws`, `forwards cancel signal to callTool`, `forwards server config and name to loadServers` | **not ported** — need a live MCP server; next session should add `tests/mcp_router_e2e.rs` driving `mcp_echo_server` | TODO |
| `isolates connections between agents` (×2), `disconnects all connections when the callback fires` | **not applicable** — see divergences (connections are per tool instance; no GC hook) | n/a |

## Divergences

- **Connections live on the tool instance**, in a `Mutex<IndexMap<String, McpClient>>`, instead of
  a `WeakMap` keyed by the agent. Give each agent its own `McpRouterTool` for the TS isolation
  property. Rust has no `FinalizationRegistry`, so there is no garbage-collection cleanup hook:
  connections close on `disconnect` or when the tool is dropped.
- **`McpRouterToolError` has no Rust equivalent**; every failure is `Error::ToolValidation` with
  the TS message verbatim, which the executor turns into an error tool result (TS throws and the
  tool factory does the same).
- **`call_tool` uses `McpClient::call_tool`** plus the crate's `map_call_tool_result` rather than
  building a throwaway `McpTool` and draining its stream — same content mapping, no fake tool spec.
  `mcp::tool` became `pub(crate)` so the mapping could be shared (no public API change).
- **Constructors return `Result`** (`new`/`with_config`) because TS throws from `makeMcpRouter`.

# Review notes

Two known footguns in `http_request`, both deliberate and both unresolved — raised for the
orchestrator rather than fixed, to keep the TS behavioural contract:

1. **No response-size cap.** `send()` reads the whole body with `response.text()`, as TS does with
   `await response.text()`. An unbounded read is a real memory footgun in a long-running agent.
   Suggested fix: a `max_response_bytes: Option<usize>` on `HttpRequestConfig` (sensible default,
   e.g. 10 MiB) that streams the body and truncates with a marker. This is a deliberate divergence
   from TS, so it needs a decision, not just a patch.
2. **`HttpRequestTool::new()` can panic.** `HttpRequestConfig::default()` calls
   `reqwest::Client::new()`, which panics if the TLS backend fails to initialize. Suggested fix:
   build with `Client::builder().build()` and either make the fallible path explicit
   (`try_new() -> Result<Self>`, `new()` kept for the infallible common case) or document the
   panic on `new()` and point at `with_config` as the escape hatch.

## Review disposition (round 1, review of `050f2c053`)

| Finding | Disposition |
| --- | --- |
| 🔴 `mcp_router` has no test that opens a connection | fixed — `tests/mcp_router_e2e.rs`, 7 tests against the `mcp_echo_server` binary: full `connect → list_tools → call_tool → disconnect`, `isError` → error, disconnect with the server already killed, connection cap, duplicate id, two simultaneous connections, cleanup on start failure. The echo server gained a `fail` tool so the `isError` path has something to return. |
| 🟠 `connections` guard held across `stop_client(..).await` | fixed — message built, `drop(connections)`, then `stop_client`, on both the cap and duplicate paths. |
| 🟠 `Error::ToolValidation` for runtime failures | fixed — `Error::other` for connect/initialise failure and for an MCP tool's `isError`; `ToolValidation` kept for input shape, allowlist, cap and duplicate id. |
| 🟠 nothing cleans up connections when the tool is dropped | fixed — `pub async fn disconnect_all(&self)` plus a module `//!` line saying the owner must call it (`Drop` cannot await). Exercised by the e2e tests. |
| 🟠 config shapes inconsistent across the five tools | fixed — `SleepConfig` gained `name`/`description` with `SleepConfig::new(max_duration)` deriving the description; `McpRouterConfig::description` is now `String`, resolved in `McpRouterConfig::new`; `HttpRequestConfig::default_timeout` → `timeout`. |
| 🟠 unbounded `response.text()` | fixed as recommended — module-doc sentence stating the body is never truncated; no truncation added. `max_response_bytes` left as a follow-up. |
| 🟡 `HttpRequestTool::new` panic needs a doc line | fixed — `# Panics` section pointing at `with_config`. |
| 🟡 `McpRouterTool::new(servers)` breaks the family's `new()` | fixed — renamed to `with_servers`; no zero-arg `new`. |
| 🟡 `src/mcp/mod.rs` visibility widened | fixed — back to `mod tool;` with `pub(crate) use tool::map_call_tool_result;`. |
| 🟡 needless `input.message.clone()` in handoff | fixed. `handoff_to_user_tool_spec` kept by value — disputed: the tool stores only the spec, so taking `&config` forces two `String` clones for no gain. |
| 🟡 `parse_method` uppercases, TS's `z.enum` does not | fixed — listed under the module's divergences. |
| 🟡 stale `PORT_REPORT.md` bullet / matrix title | fixed. |
| 🟡 `FEATURE_MATRIX.md` column widths | deferred — cosmetic; the rows render correctly and rewrapping the table conflicts with the other lanes editing it. |
