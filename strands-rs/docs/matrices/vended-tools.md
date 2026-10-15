# vended tools: `sleep`, `handoff_to_user` (`tools::vended`) — strands-rs port

Branch `agent-tasks/strands-rs-vended-tools`, worktree `/home/sandbox/work/wt/vended-tools`.
Files added: `crates/strands-agents/src/tools/vended/{sleep.rs,handoff_to_user.rs}`,
`crates/strands-agents/tests/vended_tools_loop.rs`.
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
