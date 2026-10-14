# agent-loop module — test matrix & status

## STATUS (pass B1 — `event_loop.rs`, worktree `/home/sandbox/wt/agent-loop-b`)
- Branch `agent-tasks/strands-rs-agent-loop-b`, committed. Gates clean.
  Tests: lib **520** passed, `tests/agent_loop.rs` **29** passed (15 from pass A + 14 new),
  doctests 1 passed / 2 ignored.
- Landed in pass B1 (NEXT items 1, 2, 3 and most of 6):
  - **Limits** — `InvokeOptions.limits: Option<Limits { turns, output_tokens, total_tokens }>` (all
    `Option<u64>`), `Limits::validate` (`limits.<key> must be a positive finite number, got 0`),
    per-cycle `Usage` accumulation from `StreamAggregatedResult.usage`, `Limits::check` at the top of
    each cycle with TS priority `turns > total_tokens > output_tokens` and `>=` semantics →
    `StopReason::LimitTurns / LimitTotalTokens / LimitOutputTokens`, `last_message = messages.last()`.
  - **Cancellation** — §9.4 step 14: cancelled after a ToolUse reply appends the assistant message
    **and** a user message with one `ToolResultBlock::error(id, EXECUTION_CANCELLED_MESSAGE)` per tool
    use (no orphan tool uses), stop reason `Cancelled`, `last_message` = the assistant message.
    `Err(Error::Cancelled)` from the executor now maps to `cancelled_result` (step 23) instead of
    propagating; `agent.cancel()` from inside a tool stops the loop after the results are appended.
  - **Structured output** — `InvokeOptions.structured_output_schema` +
    `Agent::structured_output::<T: DeserializeOwned + JsonSchema>()`. `StructuredOutputTool` is
    registered for the invocation and removed in **all** exit paths (step 28); a non-toolUse reply
    sets `tool_choice = Tool{strands_structured_output}` for the next cycle **without appending the
    assistant message** (step 13); a second failure → `Error::StructuredOutput("The model failed to
    invoke the structured output tool even after it was forced.")`; `_extractStructuredOutput`
    (step 20) reads `content[0].json` of the *successful* matching tool result →
    `AgentResult.structured_output`, stop reason `ToolUse`.
  - **`projected_input_tokens`** — `estimate_input_tokens` (§10 step 6): reuse the last assistant
    message carrying `metadata.usage` (`input + cache_read + cache_write + output`, plus
    `count_tokens` of the messages after it), else `count_tokens` over everything; failures leave
    `None`.
  - **Bug fixed**: model errors were degraded to `Error::Other` by `Arc::try_unwrap` after being
    shared with `AfterModelCallEvent` (the emitted event keeps a handle). `unwrap_shared_error`
    preserves `ContextWindowOverflow`, `MaxTokens`, `ModelThrottled`, `Model` and `Cancelled`.
    Without it `Error::MaxTokens` never reached the caller.
  - `Limits` + `STRUCTURED_OUTPUT_FORCED_FAILURE_MESSAGE` re-exported from `agent` and the crate
    root; `schemars`/`serde`/`tokio-util` added to the crate's `[dev-dependencies]`.

## STATUS (end of pass A)
- Worktree `/home/sandbox/wt/agent-loop`, branch `agent-tasks/strands-rs-agent-loop`, committed
  (`feat(strands-rs): agent builder, invoke/stream, core event loop + integration tests`).
- Gates clean. Tests: lib 459 passed (unchanged), `tests/agent_loop.rs` 15 passed, doctests 1 passed / 2 ignored.
- Landed:
  - `src/agent/builder.rs` — `AgentBuilder`, `Agent::builder()`, `Agent::new(model)`.
    Plugin order: conversation manager (default `SlidingWindowConversationManager`), retry strategy
    (default `DefaultModelRetryStrategy`), user plugins, session manager, `ModelPlugin(model)`.
    `.no_conversation_manager()` / `.no_retry_strategy()` opt out. Missing model →
    `Error::DefaultNotConfigured`.
  - `src/agent/event_loop.rs` — `InvokeOptions { invocation_state, cancel }`, `AgentStreamEvent`
    (15 variants + `type_name()` + `result()`), `AgentStream: Stream<Item = Result<AgentStreamEvent>>`,
    `Agent::invoke` / `invoke_with` / `stream`, `CONCURRENT_INVOCATION_MESSAGE`.
    Loop: §9.1 outer resume loop (BeforeInvocation + hook-cancel path, AfterInvocation always,
    exactly one AgentResultEvent last), §9.4 cycle (append input once → model call → ToolUse branch →
    tools → user tool-result message → `AfterToolsEvent.end_turn`), §10 `invoke_model` (retry driven
    by `AfterModelCallEvent.retry` with `attempt_count`, hook-cancel path, `Error::Cancelled` never
    retried, redaction via `_redactLastMessage`), §11 `execute_tools` (BeforeTools cancel → error
    results, empty-tool-use error, `AfterToolsEvent` always, executor events forwarded).
  - `src/agent/mod.rs` — `AgentInner` gained `plugins`, `executor`, `invoking`, `cancel`,
    `initialized`; new `Agent::{initialize, is_invoking, cancel, cancel_token, plugins, tool_executor}`.
  - `src/lib.rs` re-exports `AgentBuilder, AgentStream, AgentStreamEvent, InvokeOptions`;
    `Cargo.toml` dev-dep self-reference with `features = ["test-support"]`.
  - `crates/strands-agents/tests/agent_loop.rs` — 15 tests (below).

## NEXT (after pass B1)
1. ~~Limits~~ — **done** (pass B1).
2. ~~Cancellation~~ — **done** (pass B1), except §9.4 step 26 (consumer breaks out of the stream
   after `cancel()`): the spawned task still runs to completion, so the trailing "Cancelled by user"
   message is appended but its `MessageAddedEvent` is dropped at the channel.
3. ~~Structured output~~ — **done** (pass B1). Not covered: an *error* structured-output tool result
   (validation feedback) continuing the loop — the code path exists (`extract_structured_output`
   returns `None`), it has no test yet.
4. **`tool_caller.rs`** — owned by pass B2.
5. **Examples** — owned by pass B2.
6. Rest of the §13.4 inventory: `projected_input_tokens` and `maxTokens` propagation are **done**;
   still open: **ContextWindowOverflow + SlidingWindow interaction** (§7: overflow → manager reduces
   → `AfterModelCallEvent.retry`) — needs a test model that fails once with
   `Error::ContextWindowOverflow` and then succeeds (`MockMessageModel::Error` flattens the variant
   into `Error::Model`, so a small purpose-built test model is required); retry-strategy interaction
   tests; `AfterInvocationEvent.resume` edge cases; `AgentResult::to_string()` cases.
7. Deviations to revisit are listed below.

## Test matrix (pass B1) — `tests/agent_loop.rs` (appended)

| TS `it(...)` / spec item | Rust test | status |
|---|---|---|
| limits.turns stops the loop | `stops_when_the_turn_limit_is_reached` | pass |
| limits.outputTokens stops the loop | `stops_when_the_output_token_limit_is_reached` | pass |
| limits.totalTokens stops the loop | `stops_when_the_total_token_limit_is_reached` | pass |
| limit priority turns > tokens | `turns_limit_wins_over_token_limits` | pass |
| `_validateLimits` rejects non-positive | `rejects_non_positive_limits` | pass |
| cancel before the model call | `cancel_before_the_model_call_returns_cancelled` | pass |
| cancel after a toolUse reply (step 14) | `cancel_after_a_tool_use_reply_leaves_no_orphan_tool_uses` | pass |
| cancel during tool execution | `cancel_during_tool_execution_stops_after_the_results` | pass |
| structured output, tool called directly | `structured_output_when_the_model_calls_the_tool` | pass |
| structured output forced after text | `structured_output_is_forced_after_a_plain_text_reply` | pass |
| structured output never called → error | `structured_output_fails_when_the_model_never_calls_the_tool` | pass |
| `AgentResult.structuredOutput` via options | `structured_output_schema_is_reported_on_the_result` | pass |
| `projectedInputTokens` on BeforeModelCall | `projects_input_tokens_for_the_model_call` | pass |
| maxTokens surfaces as an error (§8.6) | `max_tokens_from_the_model_surfaces_as_an_error` | pass |

Deviation (pass B1): `Limits` fields are `Option<u64>`, so TS's "unrecognized key" and
"non-finite" `TypeError`s are unrepresentable; only the `0` case is validated. Validation happens at
the top of the cycle layer (TS `_stream` step 1), i.e. after `BeforeInvocationEvent`.

## Test matrix (pass A)
`tests/agent_loop.rs` — TS test (agent.test.ts) → Rust test → status

| TS `it(...)` | Rust test | status |
|---|---|---|
| returns the model response / basic invoke | `returns_the_model_response` | pass |
| appends input message once per invocation | `appends_input_messages_once_per_invocation` | pass |
| executes tools and continues the loop | `runs_tools_and_feeds_results_back_to_the_model` | pass |
| unknown/hallucinated tool → error result, loop continues | `unknown_tool_produces_an_error_result_and_keeps_looping` | pass |
| emits stream events in order | `emits_events_in_order` | pass |
| fires lifecycle hooks in order | `fires_lifecycle_hooks_in_order` | pass |
| BeforeInvocationEvent hook can cancel (no model call, assistant message only) | `before_invocation_hook_can_cancel_the_invocation` | pass |
| BeforeToolsEvent hook cancels the batch | `before_tools_hook_can_cancel_the_batch` | pass |
| AfterToolsEvent endTurn stops the loop | `after_tools_hook_can_end_the_turn` | pass |
| AfterInvocationEvent.resume re-invokes | `after_invocation_resume_runs_another_invocation` | pass |
| throws ConcurrentInvocationError | `rejects_a_concurrent_invocation` | pass |
| releases the lock after completion | `releases_the_invocation_lock_after_completion` | pass |
| propagates model errors | `propagates_model_errors` | pass |
| invocationState identity preserved on the result | `invocation_state_is_shared_with_the_result` | pass |
| exactly one AgentResultEvent per stream | `stream_ends_with_exactly_one_agent_result` | pass |

Skipped in pass A (reason): everything in the NEXT list — limits, cancellation paths, structured
output, tool caller, `projected_input_tokens`, conversation-manager/retry interaction, examples,
interrupts/checkpoints/middleware/telemetry (out of scope by brief).

## Deviations from TS
- **Plugin order** follows survey §8.2 step 17 (conversation manager, retry strategies, user
  plugins, session manager, `ModelPlugin`), not the shorter order quoted in the brief; the survey is
  the spec and hook order is observable.
- `ConcurrentInvocation` surfaces as the stream's first `Err` item (and as `Err` from `invoke`)
  rather than a throw at call time, because `stream()` returns `AgentStream`, not `Result<AgentStream>`.
- `stream()` sets the invoking flag synchronously, but the loop body runs in a spawned task, so the
  flag is released when the task finishes (TS releases in a `finally` on the generator).
- No layer-2/3 split (`_streamWithMiddleware` / `_streamCore`): hook dispatch happens inline in the
  `emit` helper since middleware and interrupts are out of scope. The TS "guaranteed drain"
  behaviour is approximated by the spawned task always running its cleanup emits; a consumer that
  drops the stream mid-flight gets the remaining events dropped at the channel, not re-dispatched.
- A hook error aborts the invocation (propagated as the stream's `Err`); TS's cleanup-phase
  `logger.warn` swallow path is not reproduced yet.
- `projected_input_tokens` is always `None` (`_estimateInputTokens` not ported).

## Gate output (verbatim last lines) — pass B1
- `cargo fmt` → (no output)
- `cargo clippy --all-targets --all-features -- -D warnings` → `    Finished \`dev\` profile [unoptimized] target(s) in 8.67s`
- `cargo test --all-features` → `test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 2.58s`
  (lib: `test result: ok. 520 passed; ...`; `agent_loop`: `test result: ok. 29 passed; ...`)

## pass B2 (tool caller, examples, remaining agent.test.ts tests)

Worktree `/home/sandbox/wt/agent-loop-c`, branch `agent-tasks/strands-rs-agent-loop-c`.

### Landed
- `src/agent/tool_caller.rs` — `Agent::tool(name) -> Result<ToolHandle>` (registry fuzzy `resolve`,
  `Error::ToolNotFound`), `ToolHandle::{name, call, call_with}`, `CallOptions {
  record_direct_tool_call (default true), invocation_state }`,
  `DIRECT_CALL_DURING_INVOCATION_MESSAGE`. 21 unit tests.
- `src/agent/mod.rs` — one additive `mod tool_caller;` + one `pub use` line. `src/lib.rs` — one
  `pub use agent::{CallOptions, ToolHandle};` line.
- `crates/strands-agents/examples/{basic,tools,hooks,streaming}.rs` — compile under
  `cargo build --examples --all-features` (no new dev-dependencies needed: `serde`, `schemars`,
  `tokio` (macros+rt-multi-thread), `futures` are already normal dependencies).
- `crates/strands-agents/tests/agent_loop_extra.rs` — 24 tests.

### Deviation from the task brief (deliberate)
The brief said the direct call should run through `SequentialToolExecutor` and fire
`BeforeToolCallEvent`/`AfterToolCallEvent`. TS does **not**: `tool-caller.ts` calls
`tool.stream(ctx)` itself, with **no** Before/AfterToolCall hooks and no middleware (survey §12.2
states this explicitly), and records **three** messages (assistant toolUse, user toolResult,
assistant ``agent.tool.<name> was called.``), which is what `tool-caller.test.ts` asserts. Followed
TS + survey (COMMON.md "translate, don't improve"). The toolUseId is `tooluse_<uuid v4>`.

### `tool-caller.test.ts` → Rust (21 pass, 5 skipped)
| TS `it(...)` | Rust test | status |
| --- | --- | --- |
| calls a tool by name and returns the result | `calls_a_tool_by_name_and_returns_the_result` | pass |
| calls a tool with empty input when no input provided | `calls_a_tool_with_empty_input_when_no_input_provided` | pass (`JsonValue::Null` → `{}`) |
| passes agent.cancelSignal to direct tool calls | `passes_agent_cancel_signal_to_direct_tool_calls` | pass (`Agent::cancel` is a no-op when idle, so the test marks the agent invoking first and uses `record_direct_tool_call: false`) |
| throws when tool is not found | `throws_when_tool_is_not_found` | pass (error surfaces from `Agent::tool`, not from the call) |
| resolves underscore names to hyphenated tool names | `resolves_underscore_names_to_hyphenated_tool_names` | pass |
| prefers exact name match over normalized match | — | **skipped**: `ToolRegistry::add_or_replace` rejects registering both `my_tool` and `my-tool` ("differs by a '-' or '_'"), so the TS scenario cannot be constructed in Rust |
| resolves tool names case-insensitively | `resolves_tool_names_case_insensitively` | pass |
| prefers exact match over case-insensitive match | `prefers_exact_match_over_case_insensitive_match` | pass |
| records tool call in message history by default | `records_tool_call_in_message_history_by_default` | pass (3 messages, `tooluse_` prefix) |
| does not record when recordDirectToolCall is false per-call | `does_not_record_when_record_direct_tool_call_is_false_per_call` | pass |
| records when explicitly set to true per-call | `records_when_explicitly_set_to_true_per_call` | pass |
| records full input without filtering | `records_full_input_without_filtering` | pass |
| throws ConcurrentInvocationError when agent is invoking and recording is enabled | `throws_concurrent_invocation_error_when_agent_is_invoking_and_recording_is_enabled` | pass |
| allows direct tool call during invocation when recordDirectToolCall is false | `allows_direct_tool_call_during_invocation_when_record_direct_tool_call_is_false` | pass |
| isInvoking is false on a fresh agent | `is_invoking_is_false_on_a_fresh_agent` | pass |
| propagates errors when tool throws | `propagates_errors_when_tool_throws` | pass (custom `Tool` returning `Err`; `FunctionTool` converts its handler errors into error results, as in TS) |
| is accessible as a property / returns same instance on multiple accesses / returns a ToolHandle with invoke and stream methods | — | **skipped**: the TS `Proxy` accessor has no Rust analogue; `agent.tool(name)` is a method returning a fresh handle (`uses_the_agent_cancel_token_for_the_call` covers handle construction) |
| generates unique tool use IDs using crypto.randomUUID | `generates_unique_tool_use_ids` | pass |
| yields intermediate events and returns final result / invoke() also fully consumes multi-yield generator | — | **skipped**: the Rust `Tool` trait has no `stream`; progress is an `mpsc` channel on `ToolContext`. A streaming `ToolHandle::stream` is a follow-up. |
| passes ALL parameters to tool execution | `passes_all_parameters_to_tool_execution` | pass |
| can call a tool that was added after agent creation | `can_call_a_tool_that_was_added_after_agent_creation` | pass |
| fires MessageAddedEvent for each message recorded during direct tool call | `fires_message_added_event_for_each_message_recorded_during_direct_tool_call` | pass |
| does not fire MessageAddedEvent when recordDirectToolCall is false | `does_not_fire_message_added_event_when_record_direct_tool_call_is_false` | pass |

### `tests/agent_loop_extra.rs` (24 tests, §13.4 items not in pass A's matrix)
`keeps_existing_messages_passed_to_the_builder`, `set_messages_replaces_the_conversation`,
`state_is_readable_and_writable`, `set_system_prompt_changes_what_the_model_sees`,
`fires_message_added_for_each_appended_message`,
`after_invocation_callbacks_run_in_reverse_order`, `after_model_call_callbacks_run_in_reverse_order`,
`after_invocation_resume_runs_another_invocation`,
`after_invocation_resume_is_ignored_when_the_invocation_errored`,
`rejects_concurrent_invocations`, `is_invoking_is_false_before_and_after_an_invocation`,
`initialize_is_idempotent_and_fires_initialized_once`,
`registers_tools_contributed_by_plugins`, `unknown_tool_produces_an_error_result`,
`before_tool_call_cancel_with_a_custom_reason_becomes_the_error_result`,
`before_tool_call_selected_tool_overrides_the_tool_that_runs`,
`after_tool_call_can_replace_the_result`, `after_tools_end_turn_yes_stops_the_loop`,
`after_tools_end_turn_text_appends_an_assistant_message`,
`after_tools_end_turn_blocks_appends_the_given_blocks`,
`before_tools_cancel_turns_every_result_into_an_error`,
`before_invocation_cancel_appends_only_an_assistant_message`,
`agent_result_displays_the_concatenated_text`, `stream_yields_the_agent_result_last`.

Notes:
- `initialize_is_idempotent_and_fires_initialized_once` asserts the hook fires **zero** times when
  registered after `build()` (which already initialized) and that repeated `initialize()` is a
  no-op — an `InitializedEvent`-count test on a pre-build registry needs a builder hook seam
  (`AgentBuilder` has no `hook`/`hooks` method yet). Flagged for pass C.
- `ModelPlugin` clears messages for a stateful model: **skipped** — needs a stateful mock model in
  `test_support` (`MockMessageModel` is not stateful) and the `ModelPlugin` behaviour is covered by
  its own unit tests in `src/plugins/`.
- Limits / cancel / structured-output tests intentionally left to B1.

### Gate output (verbatim last lines, pass B2)
- `cargo fmt` → (no output)
- `cargo clippy --all-targets --all-features -- -D warnings` → `    Finished \`dev\` profile [unoptimized] target(s) in 9.52s`
- `cargo test --all-features` → `test result: ok. 1 passed; 0 failed; 2 ignored; 0 measured; 0 filtered out; finished in 1.59s`
  (preceding: lib `541 passed` — includes the 21 `tool_caller` tests —, `agent_loop_extra` `24 passed`,
  `agent_loop` `15 passed` (B1's file, in flux during this run))
- `cargo build --examples --all-features` → `    Finished \`dev\` profile [unoptimized] target(s) in 0.11s`

Gotcha for the next worker: the shared target dir (`/home/sandbox/cargo-target`) served a **stale**
lib test binary while B1 was building concurrently — `cargo test --lib` reported 520 tests and
silently omitted the new `tool_caller` module. `touch crates/strands-agents/src/lib.rs` forced a
rebuild and the count went to 541. Always sanity-check the test count after a parallel build.
