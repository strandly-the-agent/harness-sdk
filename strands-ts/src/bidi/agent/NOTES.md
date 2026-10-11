# CHUNK A3 handoff — `loop.ts` is ported; most of `test_loop.py` is not

Done in A3 (`dddd294ff` implementation, `7b2f99e81` tests): `loop.ts` is a full port of
`strands-py/src/strands/bidi/agent/loop.py` — `start`/`stop`/`send`/`receive`, the reader switch
with per-content-id blocks, barge-in, response spans, usage accumulation, the tool path, and both
restart paths (reactive from `ConnectionTimeoutError`, proactive from `RestartTimer`).
`src/bidi/agent/__tests__/loop.test.ts` has 21 tests; `npx vitest run --project unit-node
src/bidi/agent` = 94 passed, `tsc --noEmit` / eslint / prettier clean.

Supporting edits made in A3:

- `agent.ts`: `_interruptState` (`new InterruptState()`), `_toolExecutor`
  (`new ConcurrentToolExecutor()`), `_tracer`, `_meter`/`_middlewareRegistry` promoted from
  `private` to internal readonly, and `_invokeCallbacks(event)` delegating to `_hooksRegistry`.
- `tools/executors/executor.ts`: `ToolExecutorOptions.agent` widened from `Agent` to the new
  `ToolExecutorAgent extends LocalAgent { _interruptState: InterruptState }`. No other call-site
  needed a change (`tsc` clean across `src/`).

## TS-specific decisions a reviewer should check against loop.py

- **One `execute()` per tool group.** `_runTools` drives a single
  `agent._toolExecutor.execute(options, input)` instead of py's `TaskGroup` of per-tool `_run_tool`s;
  `ConcurrentToolExecutor` parallelises the group. Every event the generator yields is forwarded to
  the output queue. Interrupts are rejected two ways: an `InterruptError` escaping `execute()`, and
  a non-empty `agent._interruptState.interrupts` after the group — both become
  `interrupts=<names> | tool interrupts are not supported in bidi`.
- **Cancellation is cooperative.** `TaskPool.cancel()` only aborts a signal, so the reader races
  every `reader.next()` and every queue `send()` against the pool's signal (`whenAborted`), and
  `stop()` disposes the event queue _before_ `stopAll` so a producer parked on back-pressure
  unblocks. Python relies on `asyncio.Task.cancel()` raising into the awaiting task.
- **`_restartConnection` pushes its restart event instead of `send`ing it.** A reactive restart runs
  inside `receive()`, so waiting for the receiver to ack the event would deadlock against its own
  consumer. Reader events still use `send()` (the back-pressure analog of py's `maxsize=1`).
- `connectionId` on `BidiConnectionStopEvent` is `agent.id`; py's loop has no connection id of its
  own. Revisit if a provider starts reporting one.
- Per-response time-to-first-audio is measured with `Date.now()` from the `bidiResponseStart` event.

## Tests still to port

`strands-py/tests/strands/bidi/agent/test_loop.py` — the 21 TS tests cover start (3), send (3),
receive (7), tools (3), restart (2), stop (2). Not yet ported, by area:

- **Token/usage accounting**: the `_accumulated*` / `_fold_token_baseline` / `_record_usage` tests,
  including `input_token_details.cache_read` and the baseline fold across a restart.
- **Proactive restart**: `_on_restart_warning` queueing `BidiConnectionWarningEvent`,
  `_on_restart_deadline` waiting for the turn boundary, `_await_turn_boundary`'s
  `MODEL_RESTART_TURN_TIMEOUT_S` forced swap, and `_arm_restart_timer` being a no-op when
  `autoRestart` is false or no deadline resolves. `RestartTimer` takes an injectable `sleep`, so
  these need the loop to accept one too (or `vi.useFakeTimers()`).
- **Restart guards**: `_restart_connection` declining when stopped / on a generation mismatch /
  while already restarting; `_started` re-checked after the queued restart event; the send gate
  left closed when `BidiBeforeConnectionRestartEvent` throws.
- **Generation fencing**: a superseded reader dropping its events and its `_ReaderError`;
  `_wait_for_model_task`'s bounded 2s reap logging rather than forwarding a failure.
- **Reader `finally`**: open blocks written back as incomplete when the stream ends or fails
  (the barge-in half is covered; the stream-end half is not).
- `test_loop_telemetry.py` — none of the loop-level span assertions are ported.
- `test_agent.py` loop-dependent tests — none of the 11 listed in the A2 notes are ported; the new
  `loop.test.ts` covers the tool-integration and cancel-cleanup behaviour only in outline.
