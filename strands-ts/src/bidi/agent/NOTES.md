# CHUNK A2 handoff — `loop.ts` is still a skeleton

Done in A2 (commit `98c5c28d4`): model resolution in `agent.ts` is wired to
`BedrockNovaSonicModel` (string → `{ modelId }`, undefined → default `amazon.nova-2-5-sonic`),
with the two tests (`agent.test.ts > BidiAgent > constructor > defaults to Nova Sonic…` /
`…resolves a model id string to Nova Sonic`). `npx vitest run --project unit-node src/bidi/agent`
= 73 passed; `tsc --noEmit` clean.

NOT done: `loop.ts` (`start`/`stop`/`send`/`receive` still `throw new Error('not implemented')`)
and every loop-level test. Everything below is the ported-source reading needed to write it, so
the next chunk can go straight to code.

## Port map — strands-py/src/strands/bidi/agent/loop.py (821 lines) → loop.ts

Module constants (py:64-70): `MODEL_RESTART_STOP_TIMEOUT_S = 2`, `MODEL_RESTART_WARNING_S = 10`,
`MODEL_RESTART_TURN_TIMEOUT_S = 10`. `_ReaderError` (py:73-82) is a `{ generation, error }` tag
pushed on the event queue so `receive()` can drop errors from a superseded connection.

State (py:100-147): `_started`, `_taskPool` (`async/task-pool.ts` — `create()` returns the task
promise, which is what py's `_model_task` handle needs; `cancel()` aborts + awaits), `_eventQueue`
(py uses `asyncio.Queue(maxsize=1)`; `src/queue.ts` `Queue<T>` gives the back-pressure via
`send()`/`wait()`/`shift()`+`ack()` — `put` → `await queue.send(event)`), `_invocationState`,
`_sendGate` (needs a small promise-based Event analog: `set/clear/isSet/wait`), tracer +
`_sessionSpan`, 4 `_current*` + 4 `_baseline*` token counters, `RestartTimer` (`restart-timer.ts`:
`new RestartTimer(onWarning, onDeadline)`, `arm(deadlineS, warningLeadS)`, `cancel()`),
`_restarting`, `_generation`, and turn tracking `_responseActive` / `_awaitingResponse` /
`_turnComplete` (Event, initially set).

- `start(invocationState?)` py:149-205 — throw `'loop already started | call stop before starting again'`;
  `startSessionSpan(tracer, agent.name, { modelId, tools: agent.toolNames, systemPrompt })`;
  `startConnectionSpan(tracer, { parentSpan, modelId })`; `model.start({ systemPrompt, tools:
  agent.toolRegistry.getAllToolSpecs(), messages: agent.messages })`; on throw end connection +
  session span with the error, null the session span, rethrow. Then reset token/turn state, new
  queue + pool, `_modelTask = pool.create(() => this._runModel(generation))`, default
  `invocationState.request_state = {}`, open the send gate, `_started = true`, `_armRestartTimer()`.
  Telemetry helpers all live in `src/bidi/telemetry.ts` with the option-bag signatures.
- `stop()` py:207-244 — order matters: `_started = false`, close gate, cancel timer, `_turnComplete.set()`,
  clear invocation state, `stopAll(stopTasks, stopModel)`, then **in `finally`** end the session span
  with accumulated tokens and fire `BidiAgentStopEvent` (fires even when teardown threw).
- `send(content)` py:246-276 — throw `'loop not started | call start before sending'`; await the send
  gate; a `BidiMessage` whose first block is **not** a `ToolResultBlock` is appended to history via
  `agent._appendMessages(...)` as a `user` message and sets `_awaitingResponse = true` +
  `_updateTurnState()`; then `model.send(content)`. Deltas and tool results never hit history here.
- `receive()` py:278-331 — throw `'loop not started | call start before receiving'`; loop the queue:
  stale `_ReaderError` (generation mismatch) is dropped; `ConnectionTimeoutError` surfaces to the
  caller when auto-restart is off, otherwise queues `BidiConnectionRestartEvent({ reason: 'timeout',
  timeoutError, turnInterrupted: !_turnComplete.isSet() })` and calls `_restartConnection` — if that
  throws, yield the already-queued restart event first, then rethrow. Other `Error` values raise.
  `BidiConnectionStopEvent` with `reason === 'user_request'` is yielded and ends the generator.
- `_autoRestartEnabled()` py:333-339 = `model.getConnectionConfig().autoRestart ?? true`.
  `_armRestartTimer()` py:341-352 = no-op unless enabled and `resolveDeadlineS(config)` is defined.
- `_onRestartWarning` py:354-357 queues `BidiConnectionWarningEvent({ timeLeftS })`.
  `_onRestartDeadline` py:359-377 captures the generation **before** `_awaitTurnBoundary()`, computes
  `turnInterrupted` from `_turnComplete`, restarts, and queues any failure.
  `_awaitTurnBoundary` py:379-394 waits at most `MODEL_RESTART_TURN_TIMEOUT_S`.
- `_restartConnection(timeoutError, generation, { restartEvent })` py:409-475 — the single guard point:
  declines (`false`) when stopped, when `generation !== _generation`, or when `_restarting`; else sets
  `_restarting`, cancels the timer, closes the gate, queues the restart event (re-checking `_started`
  after the suspended put), folds the token baseline, fires `BidiBeforeConnectionRestartEvent`
  (a throw propagates with the gate left closed), `_swapConnection`, resets turn state, re-arms,
  reopens the gate; `_restarting = false` in `finally`.
- `_swapConnection` py:477-510 — restart span, bump `_generation` (fences the old reader), `_restartModel`,
  `_waitForModelTask(previousReader)` (bounded 2s reap, failures logged not forwarded), start the new
  reader; end the span and fire `BidiAfterConnectionRestartEvent` (with the error) in `finally`, then
  rethrow. `_restartModel` py:512-524 — `isRestartable(model) ? model.restart(...)` else
  `model.stop()` + `model.start(...)`, forwarding `timeoutError.restartConfig`.
- Token helpers py:542-585 — `_accumulated* = baseline + current`, `_resetTokenTracking`,
  `_foldTokenBaseline`, `_recordUsage` (adds `inputTokenDetails.cache_read ?? 0`).
- `_runModel(generation)` py:587-734 — the big switch over `model.receive()`. Returns immediately
  whenever `generation !== _generation` (checked on entry, before the puts, and between puts).
  Per event: response-start rotates the response span and sets `_responseActive`; text/reasoning/
  transcript start creates the matching block from `blocks.ts` (`StreamedTextBlock`,
  `StreamedReasoningBlock`, `StreamedTranscriptBlock`) and appends its pending message (a `user`
  transcript also sets `_awaitingResponse`); audio delta records time-to-first-audio; deltas append;
  stop events pop the block, `agent._updateMessage(block.toMessage())`, and append `block.toEvent()`
  to the events emitted for that model event; `BidiToolUseBlocksEvent` appends the assistant tool-use
  message plus a placeholder tool-result message (`'Tool call started. Its result will follow in a
  separate tool exchange.'`, `metadata.custom.bidi = { kind: 'tool_dispatch' }`); barge-in adds the
  span event, clears `_responseActive`, fires `BidiBargeInEvent` hook; response-stop ends the span,
  clears both turn latches, fires `BidiResponseStopEvent` hook; usage accumulates. After the puts, a
  tool-use event spawns `_runTools`. `catch` queues a generation-tagged `_ReaderError`; `finally`
  writes every still-open block back as incomplete (`toMessage({ complete: false })`,
  `_updateMessage(..., { strict: false })`) and ends the response span with the error.
- `_runTools(toolUses)` py:736-774 — run all `_runTool`s in a `TaskGroup` (`async/task-group.ts`),
  append the assistant tool-use message + the real tool-result message
  (`metadata.custom.bidi = { kind: 'tool_result' }`), queue a `ToolResultMessageEvent`, then either
  queue `BidiConnectionStopEvent({ reason: 'user_request' })` when `agent.cancelSignal` is aborted or
  `send()` a `BidiMessage` of `ToolResultBlock`s. Any failure is queued as the error itself.
- `_runTool(toolUse)` py:776-821 — tool-call span, stream `agent._toolExecutor._stream(agent, toolUse,
  toolResults, invocationState, { structuredOutputContext: null })`, queue each tool event, reject
  `ToolInterruptEvent` with `interrupts=<names> | tool interrupts are not supported in bidi`, return
  the last `ToolResultEvent`'s result; end the span in `finally`. **Open question for the next
  chunk:** the TS `Agent` has no `_toolExecutor._stream` analog — find the equivalent
  `ConcurrentToolExecutor` / `ExecuteToolStage` call-site in `src/agent/agent.ts` (see
  `/mnt/workspace/port-notes/ts-sdk-analogs.md` §2.2–2.3) before writing this method; it is the one
  piece with no mechanical translation.

## API intel gathered in A2 (saves the next chunk the lookups)

- Hook events in `bidi/hooks/events.ts` all take an option bag and **require** `invocationState`:
  `new BidiAgentStopEvent({ agent, invocationState })`, `BidiResponseStopEvent({ …, responseId })`,
  `BidiBargeInEvent({ agent, invocationState })`,
  `BidiBeforeConnectionRestartEvent({ agent, invocationState, reason, timeoutError? })`,
  `BidiAfterConnectionRestartEvent({ agent, invocationState, reason, error? })` — note `error`,
  not py's `exception`.
- `BidiAgent._hooksRegistry` is **private**, so the loop cannot invoke callbacks yet: add an internal
  `_invokeCallbacks(event)` to `agent.ts` next to `_appendMessages` / `_updateMessage` (the core
  `Agent` has the same internal, `src/agent/agent.ts:1211`), and extend `agent.test.ts` accordingly.
- Field renames vs. Python in `bidi/types/events.ts`: `BidiConnectionStopEvent.reason` is
  `'userRequest'` (camelCase, not `'user_request'`); `BidiUsageEvent.inputTokenDetails` is optional;
  `BidiToolUseBlocksEvent.toolUses` is `ToolUseBlock[]`. `BidiConnectionRestartEvent` carries
  `{ reason, timeoutError?, turnInterrupted }`. Bidi metadata keys are camelCase too, so
  `BidiToolMetadata` kinds are `'toolDispatch'` / `'toolResult'`.
- **The tool path is the open design question.** TS has no `_tool_executor._stream(agent, tool_use,
  …)`: `src/tools/executors/executor.ts` exposes `abstract execute(options: ToolExecutorOptions,
  input: ToolExecutionInput)` where `ToolExecutionInput` is `{ toolUseBlocks, toolResultBlocks,
  invocationState, assistantMessage, completedToolResults? }` — i.e. it is per-turn (a whole group of
  tool uses), not per-tool-use, and it needs the `ToolExecutorOptions` dependency bag the core agent
  builds at its call-site. Decide first whether `_runTools` drives one `execute()` for the group
  (closer to the TS grain, and `ConcurrentToolExecutor` already parallelises) instead of py's
  per-tool-use `TaskGroup` of `_runTool`s. There is also `executeBackground(...)` which may fit the
  bidi fire-and-forget shape better. Read the `src/agent/agent.ts` call-site before writing either.
- `TaskPool.size` drops a finished task one microtask *after* its promise settles (`create()` chains
  the removal), so a test that asserts `size === 0` right after `await task` must flush a tick —
  see `async/__tests__/task-pool.test.ts`. Python's `len()` is synchronous there.

## Tests still to port

- `strands-py/tests/strands/bidi/agent/test_loop.py` — all 56, into `__tests__/loop.test.ts`
  (fake `BidiModel` with a scripted `receive()` queue; `start/stop/send/restart` as `vi.fn()`).
- `strands-py/tests/strands/bidi/agent/test_loop_telemetry.py` — the loop-level tests.
- `test_agent.py` tests that need a real loop, into `__tests__/agent.test.ts`:
  `test_bidi_agent_tool_emits_shared_hook_events_and_retries`, `test_bidi_agent_tool_injects_local_agent`,
  `test_bidi_agent_tool_context_receives_cancel_signal`, `test_bidi_agent_tool_integration`,
  `test_run_cancel_cleans_up_and_allows_reuse`, `test_send_normalizes_media` (history assertions),
  `test_send_preserves_input_identity`, `test_send_concurrent_text`,
  `test_send_list_preserves_order_and_history` (history half), `test_bidi_agent_receive_events_from_model`,
  `test_bidi_agent_start_receive_propagates_model_errors`.
- DONE in A2: `src/bidi/async/__tests__/{task-group,task-pool,stop-all}.test.ts` (9 tests) port
  `strands-py/tests/strands/bidi/_async/{test_task_group,test_task_pool,test__init__}.py`.
