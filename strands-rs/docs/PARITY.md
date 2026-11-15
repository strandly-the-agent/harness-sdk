# Parity

Every spec file of the TypeScript SDK this port tracks (`strands-ts/src/`) maps to Rust tests here. Each Rust test is named after the snake_case form of the TS `it(...)` title. A gap needs a reason (out of scope, TS-only mechanism, deprecated).

`total` is the number of test cases the TS file declares: every call rooted at `it` or `test`, including `.only`, `.skip`, `.todo`, `.concurrent` and the curried `it.each(..)(..)`, `it.for(..)(..)`, `it.skipIf(..)(..)` and `it.runIf(..)(..)` forms. A parameterized case counts once; skipped and todo cases count. Recount from the repo root (after `npm ci`) with:

```bash
node strands-rs/docs/count-ts-cases.mjs strands-ts/src
```

`*.test.browser.ts` specs don't apply to Rust.

## In scope

| TS spec | Rust tests | ported/total | gaps (with reason) |
|---|---|---|---|
| `strands-ts/src/__tests__/default-slot.test.ts` |  | 0/4 | `default-slot.ts` is used only by `sandbox/default.ts` and `mcp/config.ts`; it lands with the sandbox (Phase 8) and MCP (Phase 9) |
| `strands-ts/src/__tests__/errors.test.ts` | `strands::error::tests` | 20/29 | `isFetchNetworkError` (3) and `normalizeError` (6) are TS-only (JS `TypeError` messages and non-`Error` throwables); providers map transport failures in Phase 5 |
| `strands-ts/src/__tests__/index.test.ts` | `strands/tests/index.rs`, `strands/tests/constructors.rs`, trybuild `strands-macros/tests/ui/fail/does_not_export_the_internal_tool_executor_base.rs` | 5/9 | :21 `exports_zero_argument_tool_executors`; :34 the trybuild case (the root exports no `ToolExecutor`; `strands::tools::ToolExecutor` is the executor selector enum, not a base); :62 and :76 `exports_streaming_event_classes_as_values_not_just_types` (constructs each event); :6 error classes: `constructors.rs` builds every error class from outside the crate. Gaps: :11, :15 `BedrockModel` land with Phase 5; :45 required types (Bedrock, background tasks) with Phases 5 and 6; :38 model routing with Phase 7 |
| `strands-ts/src/__tests__/interrupt.test.ts` | `strands::interrupt::tests` | 6/26 | `InterruptError` (2), `InterruptState` behaviour, restore validation and `interruptFromAgent` (18) land with interrupt resume in Phase 4 |
| `strands-ts/src/__tests__/mime.test.ts` | `strands::mime::tests` | 6/6 | |
| `strands-ts/src/__tests__/queue.test.ts` |  | 0/10 | `queue.ts` is used only by `multiagent/graph.ts`; multi-agent graphs are out of scope for v0.1 |
| `strands-ts/src/__tests__/state-store.test.ts` | `strands::state_store::tests` | 30/49 | functions, symbols and `undefined` in values (10) and the null-key check (1) are JS runtime cases Rust types rule out (one `Serialize` failure test covers the path); typed-interface delete (1) is TS type-level only; the serializable-symbol helpers and `isStateSerializable` (7) are a TS-only mechanism |
| `strands-ts/src/agent/__tests__/agent-as-tool.interrupt.test.ts` | `strands::agent::agent_as_tool::tests::interrupt` | 9/15 | Pending: rebuilt-from-session and InterruptState-JSON turn cases (5: `resumes after both agents are rebuilt`, `persists the stored turn through InterruptState JSON`, `raises the interrupt again when the stored turn cannot be loaded`, `logs instead of re-raising ...`, `re-raises only the interrupts the stored turn still awaits`) wait for snapshots (item 4) and session managers; `resumes a context-preserving sub-agent with its own session manager` (1) waits for session managers; the no-session-manager warning part of `stores nothing for a context-preserving sub-agent` is not asserted |
| `strands-ts/src/agent/__tests__/agent-as-tool.invocation-context.test.ts` | `strands::agent::agent_as_tool::tests` | 3/3 | |
| `strands-ts/src/agent/__tests__/agent-as-tool.invocation-state.test.ts` | `strands::agent::agent_as_tool::tests` | 1/1 | |
| `strands-ts/src/agent/__tests__/agent-as-tool.test.ts` | `strands::agent::agent_as_tool::tests` | 28/31 | the four cancel-signal cases are in `tests::cancel` (the initialization case holds initialization with an async `InitializedEvent` hook instead of a plugin's `initAgent`); the three `sessionManager` cases need `SessionManager` (Phase 6). `instanceof AgentAsTool` checks read the name, description and input schema instead (no downcasting); the construction-time snapshot case mutates the wrapped agent through `with_agent`, because `as_tool` takes the agent by value |
| `strands-ts/src/agent/__tests__/agent-delegation.test.ts` | `strands::agent::agent_delegation::tests` | 16/17 | the cross-request case needs an `AfterToolsEvent` hook that throws, and hooks can't fail; the late-error-flip case flips the result in an `AfterToolCallEvent` hook (`AfterToolsEvent.message` is read-only), which reaches the same `AfterToolsEvent` re-check; the routing case runs without `backgroundTasks` (later phase) |
| `strands-ts/src/agent/__tests__/agent.cancel.test.ts` | `strands::agent::run::tests::cancel` | 26/28 | the two `cancel via stream break` cases land with the drop guard (Phase 4 item 3) |
| `strands-ts/src/agent/__tests__/agent.concurrent.test.ts` | `strands::agent::run::tests::concurrent`, `strands::agent::run::tests::invoke::runs_tools_with_the_sequential_executor`, `strands::agent::run::tests::hooks::before_tools_cancel_turns_every_call_into_an_error_result`, `strands::tools::executors::tests` | 10/22 | :201 `runs_tools_concurrently_by_default`, :181 `uses_a_reassigned_tool_executor_for_subsequent_tool_calls`, :307 `retries_one_tool_independently_from_the_other` (`run::tests::concurrent`); :224 `runs_tools_with_the_sequential_executor`; :244 `concurrent_keeps_the_event_order_of_each_tool`; :339 `an_after_hook_result_keeps_the_model_issued_id`; :372 `before_tools_cancel_turns_every_call_into_an_error_result`; :443 `a_panicking_tool_becomes_an_error_result`; :530 `reports_unknown_tools_and_tools_without_a_result`; :560 `concurrent_returns_results_in_tool_use_order`. Gaps: :148 default executor and :154 supplied instance are the `ToolExecutor` doctest in `tools/executors.rs` (no separate test); :161, :165, :173 string shorthands are compile-time (`ToolExecutor` is an enum, no strings); :405, :424, :592, :631, :661 agent cancel and consumer-break cases land with Phase 4 cancellation and the drop guard; :482 middleware failures with Phase 4 middleware; :704 interrupt telemetry with Phase 4 interrupts |
| `strands-ts/src/agent/__tests__/agent.context-manager.test.ts` | | 0/16 | |
| `strands-ts/src/agent/__tests__/agent.continuation.test.ts` | | 0/6 | all six drive the internal `continuations.addInput`, whose only producer is background tasks; continuations land with them (P-78) |
| `strands-ts/src/agent/__tests__/agent.hook.test.ts` | `strands::agent::run::tests::{hooks, end_turn_resume}`, `strands::tools::executors::tests`, `strands::conformance_tests::the_agent_loop_reproduces_the_ts_tool_run` | 66/70 | Per-tool cases: :590 retries tool call, :681 no retry when unset, :699 `BeforeToolCallEvent` on each retry, :734 retry on success: `executors::tests::retries_the_tool_when_an_after_hook_asks`; :620 no retry after cancellation: `executors::tests::cancel_signal::does_not_retry_after_the_supplied_signal_is_aborted`; :767, :800, :833 tool cancel (default text, custom text, selective): `executors::tests::a_before_hook_cancel_skips_the_tool`; :1023 `hooks::allows_retry_after_cancel_on_before_tool_call_event`; :1477 and :1539 `selectedTool` (sequential and concurrent), :1580 mutated input, :1608 renamed tool, :1635 concurrent rename: `executors::tests::a_before_hook_can_rename_or_replace_the_tool` and `tools_see_the_hooked_input_and_the_execution_signal`; :1508 `hooks::cancel_wins_over_selected_tool`. ; :620 and :652 (no retry after agent cancel, also for a hook-cancelled tool): `cancel::hook_retry`. Gaps: the `endTurn` shallow-copy case (1) holds by ownership (`EndTurn` is moved into the event); the plugin cases (3) land with Phase 4 |
| `strands-ts/src/agent/__tests__/agent.interrupt.test.ts` | `strands::agent::run::tests::interrupt` | 22/32 | Ported: tool and hook interrupts, pending storage, resume without a model call, completed siblings (incl. an in-flight sibling), hook input skipped on resume, interrupted-state rejection, multiple and multi-cycle interrupts, `AfterToolCallEvent` contract, `InterruptEvent` emission, concurrent siblings, hook-rewritten `toolUseId` (both executors in one test). Cancel cases: `interrupt::cancel`. Pending: stream break at `InterruptEvent` (3, item 3), restored state from snapshots (4, item 4). Not applicable: non-Agent `interrupt()` (1: typed events), mixed interrupt/other input (2: `InvokeArgs::InterruptResponses` is typed, so mixing doesn't compile) |
| `strands-ts/src/agent/__tests__/agent.invocation-context.test.ts` | `strands::agent::run::tests::limits::invocation_context`, `strands::agent::run::tests::structured_output::attempt_limit::counts_a_nested_invoke_failures_toward_the_request`, `strands::agent::run::tests::end_turn_resume::resume::ends_on_a_limit_stop_even_when_a_hook_asks_to_resume` | 13/16 | `rejects invalid limits` and `rejects an invocation context the SDK did not create` are compile errors (`NonZero*` limits, no public constructor); `does not throw when the invocationState bag is frozen` has no Rust counterpart (the state is a typed handle). Nested agents run through `Agent::as_tool`; one Rust-only case keeps a hand-written tool that forwards only the invocation state |
| `strands-ts/src/agent/__tests__/agent.invocation-state.test.ts` | `strands::agent::run::tests::invocation_state` | 10/11 | `preserves reference identity` is checked as shared-handle writes; the frozen-bag case doesn't apply (typed handle) |
| `strands-ts/src/agent/__tests__/agent.model-retry.test.ts` | `strands::agent::run::tests::model_retry` | 12/12 | the backoff-cancel case runs the tokio sleeper under paused time; `vi.getTimerCount()` has no counterpart |
| `strands-ts/src/agent/__tests__/agent.model-routing.test.ts` | | 0/10 | |
| `strands-ts/src/agent/__tests__/agent.stateful-model.test.ts` | | 0/12 | |
| `strands-ts/src/agent/__tests__/agent.storage.test.ts` | | 0/7 | |
| `strands-ts/src/agent/__tests__/agent.test.ts` | `strands::agent::run::tests`, `strands::agent::run::tests::{limits, structured_output, end_turn_resume}`, `strands::conformance_tests::the_agent_loop_reproduces_the_ts_tool_run`, `strands::agent::printer::tests` (printer configuration) | 104/147 | stream, invoke, traces, metrics, API consistency, messages, nested tools, system prompt, model property, tracking ids, multimodal input, aux model, redaction, token estimate, tool-name and empty-message normalization, limits (12) and structured output (27), printer configuration (4) are ported; the two `AsyncGenerator` type cases are compile-time. Cycle duration (`run::tests::cycle_duration`, 2), `_estimateInputTokens` (`run::tests::estimate_input_tokens`, 6 with the 5 added in the review pass) and `supports all ContentBlock types` (`run::tests::input`) are ported. limits `invalid` and `unrecognized key` (2) are compile errors (`NonZero*` fields, a struct); structured output across an interrupt resume (2) with Phase 4, checkpoint (1) out of scope, context-manager offloading (3) with Phase 6 (the `disabled` row of the offloading table is ported); the consumer-break and cancel cases (2) with Phase 4; hook-error cleanup (5) can't happen (hooks return `()`); concurrency guards (3) are compile errors (`&mut self`); string/`BedrockModel` model ids (8) with Phase 5; `ModelRouter` (1) with Phase 7; `sessionId`/`agentMetadata`/`shutdown` flush (10) with Phase 6; MCP `toolsChanged` (1) with Phase 9; `ContentBlockData`/`MessageData`/`SystemPromptData` inputs (6) don't apply to typed Rust input; injected blocks (1) are provider formatting (Phase 5) |
| `strands-ts/src/agent/__tests__/agent.tracer.test.node.ts` | | 0/47 | |
| `strands-ts/src/agent/__tests__/invocation-context.test.ts` | `strands::agent::invocation_context::tests` | 2/2 | |
| `strands-ts/src/agent/__tests__/printer.test.ts` | `strands::agent::printer::tests` | 9/9 | the tests swap in a capturing printer through the crate-internal `Agent::printer` field, as TS assigns `_printer` |
| `strands-ts/src/agent/__tests__/snapshot.test.ts` | | 0/32 | |
| `strands-ts/src/agent/__tests__/tool-caller.test.ts` | `strands::agent::tool_caller::tests` | 20/26 | the three concurrency cases are compile-time (`agent.tool(..)` borrows the agent mutably, so a recorded call can't overlap an invocation and there is no `isInvoking`; unrecorded calls from inside a tool are `ToolAgent::call_tool`, Phase 12); the three `agent.tool` accessor cases test the TS `Proxy` (same instance, `invoke`/`stream` present), which the compiler checks |
| `strands-ts/src/background-tasks/__tests__/background-tasks.test.ts` | | 0/9 | |
| `strands-ts/src/background-tasks/in-process/__tests__/engine-execution.test.ts` | | 0/5 | |
| `strands-ts/src/background-tasks/in-process/__tests__/engine-transitions.test.ts` | | 0/7 | |
| `strands-ts/src/background-tasks/in-process/__tests__/manager.test.ts` | | 0/9 | |
| `strands-ts/src/context-manager/__tests__/context-manager.test.ts` | | 0/20 | |
| `strands-ts/src/context-manager/__tests__/presets.test.ts` | | 0/17 | |
| `strands-ts/src/context-manager/__tests__/retrieval-tool.test.ts` | | 0/9 | |
| `strands-ts/src/context-manager/__tests__/stash-integration.test.ts` | | 0/17 | |
| `strands-ts/src/context-manager/__tests__/stash.test.ts` | | 0/16 | |
| `strands-ts/src/context-manager/strategies/__tests__/offload-strategy.test.ts` | | 0/54 | |
| `strands-ts/src/context-manager/strategies/__tests__/summarize-strategy.test.ts` | | 0/55 | |
| `strands-ts/src/conversation-manager/__tests__/agentic-context.test.ts` | | 0/30 | |
| `strands-ts/src/conversation-manager/__tests__/context-compression.test.ts` | | 0/32 | |
| `strands-ts/src/conversation-manager/__tests__/conversation-manager.test.ts` | | 0/19 | |
| `strands-ts/src/conversation-manager/__tests__/null-conversation-manager.test.ts` | | 0/4 | |
| `strands-ts/src/conversation-manager/__tests__/pin.test.ts` | | 0/16 | |
| `strands-ts/src/conversation-manager/__tests__/sliding-window-conversation-manager.test.ts` | | 0/65 | |
| `strands-ts/src/conversation-manager/__tests__/summarizing-conversation-manager.test.ts` | | 0/20 | |
| `strands-ts/src/conversation-manager/__tests__/token-usage-middleware.test.ts` | | 0/15 | |
| `strands-ts/src/hooks/__tests__/events.test.ts` | `strands::hooks::events::tests`, `strands::types::agent::stream_event::tests` | 69/71 | `toJSON serialization completeness` (2) reflects over TS instance keys, which Rust records don't have (their fields are fixed by type) |
| `strands-ts/src/hooks/__tests__/registry.test.ts` | `strands::hooks::registry::tests` | 21/25 | `propagates callback errors`, `stops execution on first non-interrupt error`, `stops on non-interrupt error even when interrupts were collected` (3): Rust hook callbacks return `()` and can't fail; `cleanup function is idempotent` (1): `HookCleanup::remove` takes `self`, so a second call doesn't compile |
| `strands-ts/src/injection/__tests__/message-injection.test.ts` | | 0/31 | |
| `strands-ts/src/interventions/__tests__/handler.test.ts` | | 0/2 | |
| `strands-ts/src/interventions/__tests__/registry.test.ts` | | 0/44 | |
| `strands-ts/src/logging/__tests__/logger.test.ts` | | 0/6 | |
| `strands-ts/src/logging/__tests__/warn-once.test.ts` | `strands::logging::warn_once::tests` | 3/3 | |
| `strands-ts/src/mcp/__tests__/client-annotations.test.ts` | | 0/4 | |
| `strands-ts/src/mcp/__tests__/client-progress.test.ts` | | 0/2 | |
| `strands-ts/src/mcp/__tests__/client.test.ts` | | 0/126 | |
| `strands-ts/src/mcp/__tests__/config.test.node.ts` | | 0/48 | |
| `strands-ts/src/memory/__tests__/memory-manager.test.ts` | | 0/81 | |
| `strands-ts/src/memory/__tests__/memory-tracing.test.ts` | | 0/17 | |
| `strands-ts/src/memory/extraction/__tests__/extraction.test.ts` | | 0/42 | |
| `strands-ts/src/memory/extraction/__tests__/model-extractor.test.ts` | | 0/14 | |
| `strands-ts/src/memory/extraction/__tests__/resolve-extraction-config.test.ts` | | 0/13 | |
| `strands-ts/src/middleware/__tests__/agent-middleware.test.ts` | | 0/62 | |
| `strands-ts/src/middleware/__tests__/copy-on-input.test.ts` | | 0/21 | |
| `strands-ts/src/middleware/__tests__/custom-stages.test.ts` | | 0/5 | |
| `strands-ts/src/middleware/__tests__/middleware-interrupts.test.ts` | | 0/15 | |
| `strands-ts/src/middleware/__tests__/registry.test.ts` | | 0/24 | |
| `strands-ts/src/models/__tests__/anthropic.test.ts` | | 0/186 | |
| `strands-ts/src/models/__tests__/bedrock.test.ts` | | 0/274 | |
| `strands-ts/src/models/__tests__/cache-portability.test.ts` | | 0/1 | every case builds Bedrock and Anthropic requests; ported with the providers in Phase 5 |
| `strands-ts/src/models/__tests__/defaults.test.ts` | `strands::models::defaults::tests` | 7/7 | |
| `strands-ts/src/models/__tests__/model-info-lookup.test.ts` | `strands::models::model_info::tests` | 9/9 | |
| `strands-ts/src/models/__tests__/model-info.test.node.ts` | `strands::models::model_info::tests` | 1/1 | |
| `strands-ts/src/models/__tests__/model-info.test.ts` | `strands::models::model_info::tests` | 4/5 | `returns a frozen record` (1) is a TS-only mechanism (`Object.freeze`); Rust returns an owned value |
| `strands-ts/src/models/__tests__/model-proxy.test.ts` | `strands::models::model_proxy::tests` | 19/19 | `toBe` identity checks compare by value |
| `strands-ts/src/models/__tests__/model.test.ts` | `strands::models::model::tests`, `strands::models::summarize::tests` | 70/70 | |
| `strands-ts/src/models/__tests__/native-output.test.ts` | `strands::models::native_output::tests` | 15/15 | |
| `strands-ts/src/models/__tests__/server-side-summary-capability.test.ts` | `strands::models::model::tests::info` | 1/2 | the provider cases (1) need Bedrock, OpenAI, Google and Vercel providers (Phase 5); the ported case covers the model half, its `Offload` half lands in Phase 6 |
| `strands-ts/src/models/__tests__/streaming.test.ts` | `strands::models::streaming::tests` | 14/14 | |
| `strands-ts/src/models/__tests__/strict-schema.test.ts` | `strands::models::strict_schema::tests` | 24/24 | |
| `strands-ts/src/models/openai/__tests__/chat.test.ts` | | 0/96 | |
| `strands-ts/src/models/openai/__tests__/errors.test.ts` | | 0/7 | |
| `strands-ts/src/models/openai/__tests__/mantle.test.ts` | | 0/30 | |
| `strands-ts/src/models/openai/__tests__/responses-cache.test.ts` | | 0/20 | |
| `strands-ts/src/models/openai/__tests__/responses.test.ts` | | 0/72 | |
| `strands-ts/src/models/routing/__tests__/classifier-strategy.test.ts` | | 0/25 | |
| `strands-ts/src/models/routing/__tests__/fallback-strategy.test.ts` | | 0/5 | |
| `strands-ts/src/models/routing/__tests__/router.test.ts` | | 0/22 | |
| `strands-ts/src/plugins/__tests__/plugin.test.ts` | | 0/7 | |
| `strands-ts/src/plugins/__tests__/registry.test.ts` | | 0/10 | |
| `strands-ts/src/registry/__tests__/tool-registry.test.ts` | `strands::tools::registry::tests` | 31/33 | `throws ToolValidationError for a non-string tool name`, `allows a tool with a null or undefined description` (2): `Tool::name`/`description` return `&str` |
| `strands-ts/src/retry/__tests__/backoff-strategy.test.ts` | `strands::retry::backoff_strategy::tests` | 15/19 | the attempt checks (`rejects attempts below 1` x3, `rejects non-integer attempts`) are ruled out by `BackoffContext::attempt: NonZeroU32` |
| `strands-ts/src/retry/__tests__/default-model-retry-strategy.test.ts` | `strands::retry::default_model_retry_strategy::tests`, `strands::agent::run::tests::model_retry::the_strategy_registers_an_after_model_call_hook_at_initialize` | 14/15 | `rejects maxAttempts below 1` (1) is ruled out by `max_attempts: NonZeroU32` |
| `strands-ts/src/sandbox/__tests__/default.test.node.ts` | | 0/4 | |
| `strands-ts/src/sandbox/__tests__/errors.test.node.ts` | `strands::error::tests` | 9/9 | |
| `strands-ts/src/sandbox/__tests__/not-a-sandbox-local-environment.test.node.ts` | | 0/27 | |
| `strands-ts/src/sandbox/__tests__/posix-shell.test.node.ts` | | 0/55 | |
| `strands-ts/src/sandbox/__tests__/read-file-errors.test.node.ts` | | 0/6 | |
| `strands-ts/src/sandbox/__tests__/stream-process.test.node.ts` | | 0/5 | |
| `strands-ts/src/session/__tests__/session-manager.test.ts` | | 0/71 | |
| `strands-ts/src/session/__tests__/snapshot-storage-adapter.test.ts` | | 0/24 | |
| `strands-ts/src/session/__tests__/validation.test.ts` | | 0/10 | |
| `strands-ts/src/storage/__tests__/in-memory-storage.test.ts` | | 0/25 | |
| `strands-ts/src/storage/__tests__/local-file-storage.test.node.ts` | | 0/31 | |
| `strands-ts/src/storage/__tests__/namespaced-storage.test.ts` | | 0/10 | |
| `strands-ts/src/storage/search/__tests__/keyword.test.ts` | | 0/13 | |
| `strands-ts/src/telemetry/__tests__/config.test.node.ts` | | 0/18 | |
| `strands-ts/src/telemetry/__tests__/config.test.ts` | | 0/6 | |
| `strands-ts/src/telemetry/__tests__/json.test.ts` | | 0/15 | |
| `strands-ts/src/telemetry/__tests__/local-trace.test.ts` | | 0/16 | |
| `strands-ts/src/telemetry/__tests__/meter.test.ts` | | 0/62 | |
| `strands-ts/src/telemetry/__tests__/tracer.test.node.ts` | | 0/100 | |
| `strands-ts/src/tools/__tests__/structured-output-tool.test.ts` | `strands::tools::structured_output::tests` | 11/12 | `returns error result for non-ZodError exceptions` mocks zod's `parse`; Rust validation can't throw |
| `strands-ts/src/tools/__tests__/tool-factory.test.ts` | `strands::tools::function_tool::tests`, `strands::tools::macro_support::tests` | 5/8 | :8 Zod schema: `function_tool::tests::typed::generates_the_input_schema_from_the_type` (`FunctionTool::typed`); :20 plain-object schema: `properties::a_raw_schema_is_sent_unchanged`; :33 omitted schema: `properties::creates_tool_with_default_empty_object_schema`; :49 and :63: `macro_support::tests::handles_synchronous_callback`, `handles_promise_callback`. Gaps: :74 async generator callbacks are a TS-only mechanism (streaming tools implement `Tool`); :91 `instanceof Tool` is a compile-time trait bound; :102 description defaulting to `''` doesn't apply: `#[tool]` without a doc comment is a compile error (P-57) and `FunctionTool::new` takes the description |
| `strands-ts/src/tools/__tests__/tool.test.ts` | `strands::tools::tool::tests`, `strands::tools::function_tool::tests` | 22/46 | async generator callbacks (TS-only mechanism; streaming tools implement `Tool`), deep-copy and circular-reference cases (owned acyclic values), non-`Error` throws and stack traces, `instanceof` and interface-shape cases (no Rust form); remaining `isValidToolName` table rows are covered by one table test |
| `strands-ts/src/tools/__tests__/zod-tool.test.ts` | `strands::tools::macro_support::tests`, `strands::tools::function_tool::tests::typed`, `strands::tools::schema::tests` | 24/32 | Ported through `#[tool]`, `#[tools]` and `FunctionTool::typed` (`invoke()` cases through `FunctionTool::typed`). Gaps: async generator callbacks :85, :189, :524, :598 (4) are a TS-only mechanism (streaming tools implement `Tool`); the second `z.void()` group :541, :562, :575 (3) has no Rust form (a tool without parameters covers the first group); :630 Zod conversion errors (1) have no Rust form (schemars output always converts) |
| `strands-ts/src/tools/executors/__tests__/executor.cancel-signal.test.ts` | `strands::tools::executors::tests::cancel_signal` | 2/3 | `keeps middleware and tools on the supplied execution signal` (1) needs middleware (Phase 4); the tool half is covered by `tools_see_the_hooked_input_and_the_execution_signal`. The stream-consumer `event.retry = true` is ported as an `AfterToolCallEvent` hook, since stream records are read-only (section 8.6) |
| `strands-ts/src/types/__tests__/agent.test.ts` | `strands::types::agent::tests::{agent_result_to_json, agent_result}` | 26/30 | the 4 `checkpoint` cases are out of scope (checkpointing is out of v0.1) |
| `strands-ts/src/types/__tests__/citations.test.ts` | `strands::types::citations::tests` | 6/6 | |
| `strands-ts/src/types/__tests__/json.test.ts` | | 0/44 | TS-only: `deepCopy`/`deepCopyWithValidation` clone JS values and reject functions, symbols, `undefined` and cycles, none of which a Rust `JsonValue` can hold (`clone()` is the deep copy) |
| `strands-ts/src/types/__tests__/media.test.ts` | `strands::types::media::tests` | 51/51 | |
| `strands-ts/src/types/__tests__/messages.test.ts` | `strands::types::messages::tests` | 84/86 | `returns them unchanged` (systemPromptFromData with class instances) and `keeps an InjectedBlock passed inside message data an InjectedBlock` are TS-only (class instances inside plain data) |
| `strands-ts/src/types/__tests__/validation.test.ts` | | 0/5 | TS-only: `ensureDefined` guards `null`/`undefined`, which Rust expresses with `Option` |
| `strands-ts/src/vended-interventions/cedar/__tests__/cedar.test.node.ts` | | 0/39 | |
| `strands-ts/src/vended-interventions/hitl/__tests__/hitl.test.ts` | | 0/36 | |
| `strands-ts/src/vended-memory-stores/file-memory-store/__tests__/file-memory-store.test.node.ts` | | 0/52 | |
| `strands-ts/src/vended-plugins/context-injector/__tests__/plugin.test.ts` | | 0/19 | |
| `strands-ts/src/vended-plugins/skills/__tests__/agent-skills.test.node.ts` | | 0/39 | |
| `strands-ts/src/vended-plugins/skills/__tests__/skill.test.node.ts` | | 0/46 | |
| `strands-ts/src/vended-tools/__tests__/paths.test.ts` | | 0/7 | |
| `strands-ts/src/vended-tools/bash/__tests__/bash.test.node.ts` | | 0/46 | |
| `strands-ts/src/vended-tools/edit/__tests__/edit.test.node.ts` | | 0/13 | |
| `strands-ts/src/vended-tools/handoff-to-user/__tests__/handoff-to-user.test.ts` | | 0/9 | |
| `strands-ts/src/vended-tools/http-request/__tests__/http-request.test.ts` | | 0/12 | |
| `strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.node.ts` | | 0/1 | |
| `strands-ts/src/vended-tools/mcp-router/__tests__/mcp-router.test.ts` | | 0/28 | |
| `strands-ts/src/vended-tools/notebook/__tests__/notebook.test.ts` | | 0/62 | |
| `strands-ts/src/vended-tools/python-repl/__tests__/python-repl.test.node.ts` | | 0/12 | |
| `strands-ts/src/vended-tools/read/__tests__/read.test.node.ts` | | 0/34 | |
| `strands-ts/src/vended-tools/shell/__tests__/shell.test.node.ts` | | 0/18 | |
| `strands-ts/src/vended-tools/sleep/__tests__/sleep.test.ts` | | 0/19 | |
| `strands-ts/src/vended-tools/stop/__tests__/stop.test.ts` | | 0/18 | |
| `strands-ts/src/vended-tools/subagent/__tests__/subagent.test.ts` | | 0/33 | |
| `strands-ts/src/vended-tools/web-fetch/__tests__/extract.test.ts` | | 0/15 | |
| `strands-ts/src/vended-tools/web-fetch/__tests__/web-fetch.test.ts` | | 0/30 | |
| `strands-ts/src/vended-tools/write/__tests__/write.test.node.ts` | | 0/11 | |

## Out of scope

| TS spec | reason |
|---|---|
| `strands-ts/src/a2a/__tests__/a2a-agent.test.ts` | multi-agent and A2A are out of v0.1 |
| `strands-ts/src/a2a/__tests__/adapters.test.ts` | multi-agent and A2A are out of v0.1 |
| `strands-ts/src/a2a/__tests__/async-lock.test.ts` | multi-agent and A2A are out of v0.1 |
| `strands-ts/src/a2a/__tests__/events.test.ts` | multi-agent and A2A are out of v0.1 |
| `strands-ts/src/a2a/__tests__/executor.test.ts` | multi-agent and A2A are out of v0.1 |
| `strands-ts/src/a2a/__tests__/server.test.node.ts` | multi-agent and A2A are out of v0.1 |
| `strands-ts/src/a2a/__tests__/server.test.ts` | multi-agent and A2A are out of v0.1 |
| `strands-ts/src/agent/__tests__/agent.checkpoint.test.ts` | checkpointing is out of v0.1 |
| `strands-ts/src/decisions/__tests__/decision-model.test.ts` | decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/llm-decision-model.test.ts` | decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/llm-decision-model.tracing.test.node.ts` | decision models are out of v0.1 |
| `strands-ts/src/decisions/__tests__/system-one-decision-models.test.node.ts` | decision models are out of v0.1 |
| `strands-ts/src/experimental/__tests__/checkpoint.test.ts` | checkpointing is out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-client-legacy.test.node.ts` | MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-client-stdio.test.node.ts` | MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-client.test.ts` | MCP tasks are out of v0.1 |
| `strands-ts/src/mcp/__tests__/task-transport.test.ts` | MCP tasks are out of v0.1 |
| `strands-ts/src/models/__tests__/google.test.ts` | the Google provider is out of v0.1 |
| `strands-ts/src/models/__tests__/vercel.test.ts` | the Vercel provider is out of v0.1 |
| `strands-ts/src/models/routing/__tests__/classifier-strategy.decision-model.test.ts` | decision-model HITL classifier is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/agent-tool-streamer.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/events.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/graph.invocation-context.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/graph.invocation-state.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/graph.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/graph.tracer.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/interrupts.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/nodes.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/plugins.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/snapshot.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/spec.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/state.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/swarm.invocation-context.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/swarm.invocation-state.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/swarm.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/multiagent/__tests__/swarm.tracer.test.ts` | multi-agent is out of v0.1 |
| `strands-ts/src/sandbox/__tests__/docker.test.node.ts` | Docker sandbox is out of v0.1 |
| `strands-ts/src/sandbox/__tests__/ssh.test.node.ts` | SSH sandbox is out of v0.1 |
| `strands-ts/src/session/__tests__/file-storage.test.node.ts` | deprecated session FileStorage |
| `strands-ts/src/session/__tests__/s3-storage.test.ts` | S3 storage is out of v0.1 |
| `strands-ts/src/storage/__tests__/s3-storage.test.ts` | S3 storage is out of v0.1 |
| `strands-ts/src/storage/embeddings/__tests__/bedrock.test.ts` | embeddings are out of v0.1 |
| `strands-ts/src/storage/search/__tests__/s3-vector.test.ts` | vector search is out of v0.1 |
| `strands-ts/src/vended-interventions/steering/__tests__/handler.test.ts` | steering is out of v0.1 |
| `strands-ts/src/vended-interventions/steering/__tests__/llm.test.ts` | steering is out of v0.1 |
| `strands-ts/src/vended-interventions/steering/__tests__/tool-ledger.test.ts` | steering is out of v0.1 |
| `strands-ts/src/vended-memory-stores/bedrock-knowledge-base/__tests__/bedrock-knowledge-base-store.test.node.ts` | Bedrock Knowledge Base memory store is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/file-storage-sandbox.test.node.ts` | the ContextOffloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/plugin.test.node.ts` | the ContextOffloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/plugin.test.ts` | the ContextOffloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/search.test.ts` | the ContextOffloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/storage.test.node.ts` | the ContextOffloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/context-offloader/__tests__/storage.test.ts` | the ContextOffloader plugin is out of v0.1 |
| `strands-ts/src/vended-plugins/goal/__tests__/plugin.test.ts` | GoalLoop is out of v0.1 |
| `strands-ts/src/vended-tools/a2a-client/__tests__/a2a-client.test.ts` | a2a_client tool is out of v0.1 |
| `strands-ts/src/vended-tools/file-editor/__tests__/file-editor.test.node.ts` | deprecated fileEditor |
| `strands-ts/src/vended-tools/graph/__tests__/graph.test.ts` | graph tool is out of v0.1 |
| `strands-ts/src/vended-tools/swarm/__tests__/swarm.test.ts` | swarm tool is out of v0.1 |
| `strands-ts/src/storage/search/__tests__/qmd.test.node.ts` | QMD (SQLite BM25 via `@tobilu/qmd`) is out of v0.1; `KeywordSearchStrategy` is the v0.1 search strategy |
| `strands-ts/src/vended-memory-stores/test-memory-store/__tests__/test-memory-store.test.node.ts` | `TestMemoryStore` is not in the v0.1 scope; `FileMemoryStore` is the v0.1 local store |

## Live checklist

The TS integration specs (`strands-ts/test/integ/`) run against real providers. In-scope ones are run as Rust live tests (see `TESTING.md`), not ported as unit tests.

| TS spec | status |
|---|---|
| `strands-ts/test/integ/a2a/a2a-agent.test.ts` | out of scope: A2A is out of v0.1 |
| `strands-ts/test/integ/a2a/express-server.test.node.ts` | out of scope: A2A is out of v0.1 |
| `strands-ts/test/integ/agent-as-tool.test.ts` | live |
| `strands-ts/test/integ/agent.cancel.test.ts` | live |
| `strands-ts/test/integ/agent.checkpoint.test.node.ts` | out of scope: checkpointing is out of v0.1 |
| `strands-ts/test/integ/agent.test.ts` | live |
| `strands-ts/test/integ/cedar.test.node.ts` | live |
| `strands-ts/test/integ/conversation-manager/agentic-context.test.ts` | live |
| `strands-ts/test/integ/conversation-manager/context-manager-message-reduction.test.ts` | live |
| `strands-ts/test/integ/conversation-manager/context-manager-summarize-tool-calls.test.ts` | live |
| `strands-ts/test/integ/conversation-manager/summarizing-conversation-manager.test.ts` | live |
| `strands-ts/test/integ/decisions/decision-model.test.ts` | out of scope: decision models are out of v0.1 |
| `strands-ts/test/integ/decisions/system-one-decision-models.test.node.ts` | out of scope: decision models are out of v0.1 |
| `strands-ts/test/integ/environment.test.node.ts` | TS-only: checks the JavaScript runtime and environment detection |
| `strands-ts/test/integ/environment.test.ts` | TS-only: checks JavaScript features and the TypeScript configuration |
| `strands-ts/test/integ/goal/goal.test.node.ts` | out of scope: GoalLoop is out of v0.1 |
| `strands-ts/test/integ/interrupt.test.ts` | live |
| `strands-ts/test/integ/interventions.test.ts` | live |
| `strands-ts/test/integ/mcp/mcp-connect-abort.test.node.ts` | live |
| `strands-ts/test/integ/mcp/mcp-legacy-fallback.test.node.ts` | live |
| `strands-ts/test/integ/mcp/mcp-reconnect.test.node.ts` | live |
| `strands-ts/test/integ/mcp/mcp-sep2663.test.node.ts` | out of scope: MCP tasks are out of v0.1 |
| `strands-ts/test/integ/mcp/mcp-tasks-stdio.test.node.ts` | out of scope: MCP tasks are out of v0.1 |
| `strands-ts/test/integ/mcp/mcp-tasks.test.node.ts` | out of scope: MCP tasks are out of v0.1 |
| `strands-ts/test/integ/mcp/mcp-v2.test.node.ts` | live |
| `strands-ts/test/integ/mcp/mcp.test.node.ts` | live |
| `strands-ts/test/integ/memory/bedrock-knowledge-base-memory-manager.test.node.ts` | out of scope: the Bedrock Knowledge Base memory store is out of v0.1 |
| `strands-ts/test/integ/memory/bedrock-knowledge-base-store.test.node.ts` | out of scope: the Bedrock Knowledge Base memory store is out of v0.1 |
| `strands-ts/test/integ/models/anthropic.test.ts` | live |
| `strands-ts/test/integ/models/bedrock.test.node.ts` | live |
| `strands-ts/test/integ/models/bedrock.test.ts` | live |
| `strands-ts/test/integ/models/classifier-strategy.test.ts` | live |
| `strands-ts/test/integ/models/context-windows.test.ts` | live for Anthropic; the Google rows are out of scope (Google provider is out of v0.1) |
| `strands-ts/test/integ/models/google.test.ts` | out of scope: the Google provider is out of v0.1 |
| `strands-ts/test/integ/models/model-smoke.test.ts` | live for Bedrock, Bedrock Mantle, Anthropic and OpenAI; the Google rows are out of scope (Google provider is out of v0.1) |
| `strands-ts/test/integ/models/openai/chat.test.ts` | live |
| `strands-ts/test/integ/models/openai/mantle-routing.test.node.ts` | live |
| `strands-ts/test/integ/models/openai/mantle.test.node.ts` | live |
| `strands-ts/test/integ/models/openai/responses.test.ts` | live |
| `strands-ts/test/integ/models/openai/session-cache-key.test.ts` | live |
| `strands-ts/test/integ/multiagent/graph.test.ts` | out of scope: multi-agent is out of v0.1 |
| `strands-ts/test/integ/multiagent/interrupt-hook.test.node.ts` | out of scope: multi-agent is out of v0.1 |
| `strands-ts/test/integ/multiagent/interrupt-node.test.node.ts` | out of scope: multi-agent is out of v0.1 |
| `strands-ts/test/integ/multiagent/interrupt-session.test.node.ts` | out of scope: multi-agent is out of v0.1 |
| `strands-ts/test/integ/multiagent/session-manager.test.node.ts` | out of scope: multi-agent is out of v0.1 |
| `strands-ts/test/integ/multiagent/swarm.test.ts` | out of scope: multi-agent is out of v0.1 |
| `strands-ts/test/integ/qmd-search-strategy.test.node.ts` | out of scope: QMD search is out of v0.1 |
| `strands-ts/test/integ/s3-vector-search-strategy.test.node.ts` | out of scope: S3 vector search is out of v0.1 |
| `strands-ts/test/integ/sandbox/docker.test.node.ts` | out of scope: Docker sandboxes are out of v0.1 |
| `strands-ts/test/integ/sandbox/ssh.test.node.ts` | out of scope: SSH sandboxes are out of v0.1 |
| `strands-ts/test/integ/session-manager.test.node.ts` | live for the stateful-model (OpenAI Responses) cases; the deprecated session `FileStorage` and `S3Storage` cases are out of scope |
| `strands-ts/test/integ/skills/agent-skills.test.node.ts` | live |
| `strands-ts/test/integ/telemetry.test.node.ts` | live |
| `strands-ts/test/integ/tools/bash.test.node.ts` | live |
| `strands-ts/test/integ/tools/file-editor.test.node.ts` | out of scope: deprecated fileEditor |
| `strands-ts/test/integ/tools/http-request.test.ts` | live |
| `strands-ts/test/integ/tools/notebook.test.ts` | live |
| `strands-ts/test/integ/vended-interventions/steering/steering.test.node.ts` | out of scope: steering is out of v0.1 |
