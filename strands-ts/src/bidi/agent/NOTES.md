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
- `connectionId` on `BidiConnectionStopEvent` is `agent.id`; py reports the model's
  `_connection_id` (falling back to `"unknown"`). Still open — no TS `BidiModel` exposes a
  connection id yet.
- **No `ToolResultMessageEvent` in TS core.** Python queues `strands.types._events`
  `ToolResultMessageEvent` after a tool group's results are recorded; TS core has no counterpart,
  so `_runTools` queues `MessageAddedEvent` (the TS core event documented for tool-result messages)
  with the same message. Consumers switching on the event name see `messageAddedEvent`.
- **No `MessageUpdatedEvent` in TS core.** `_updateMessage` therefore fires nothing where
  `agent.py:700` fires it on every streamed completion and incomplete-marking. Gap, not parity.
- The connection span ends as soon as `model.start()` succeeds (loop.py:190); it measures the
  connect, not the session.
- `_onRestartWarning` pushes its event instead of ack-waiting `send()` — an ack-wait from the
  restart timer stalls the proactive restart behind an idle receiver. Python's `maxsize=1` queue
  back-pressures the warning instead, so `test_connection_events_share_bounded_event_queue` was
  ported with the FIFO assertion kept and the "warning blocks" assertion dropped.
- `run()` drives `_runInputs`/`_runOutputs` through `TaskGroup` (py `_TaskGroup`), and
  `_runOutputs` iterates `receive()` as a raw iterator so a sibling failure can abandon a read
  parked on the event queue.
- Per-response time-to-first-audio is measured with `Date.now()` from the `bidiResponseStart` event.

## Tests still to port

`src/bidi/agent/__tests__/loop.test.ts` has 29 tests (`npx vitest run --project unit-node
src/bidi/agent` = 102 passed). The seven tests the loop review named are ported:

| `test_loop.py` | `loop.test.ts` |
| --- | --- |
| `test_restart_fences_superseded_reader_stream_close_error` | restart > drops the stream-close error raised by a superseded reader |
| `test_stale_reader_event_does_not_corrupt_state_across_restart` | restart > counts usage once per event across a restart |
| `test_deadline_callback_does_not_restart_after_stop_while_queue_full` | restart > does not restart the model when the deadline fires after stop |
| `test_connection_events_share_bounded_event_queue` | event queue > delivers connection events in order on the shared queue |
| `test_tool_exchanges_remain_paired_when_results_finish_out_of_order` | tools > keeps tool exchanges paired when groups finish out of order (+ records a tool group results in tool-use request order) |
| `test_agent_stop_hook[cleanup_fails]` | stop > fires the agent stop hook even when cleanup fails |
| `test_reactive_restart_failure_yields_event_before_raising` | restart > yields the restart event before surfacing a failed reactive restart |

Still not ported, by area:

- **Token/usage accounting**: the `_fold_token_baseline` / `_record_usage` unit tests, including
  `input_token_details.cache_read`. The cross-restart fold is now covered end to end.
- **Proactive restart**: `_on_restart_deadline` waiting for the turn boundary, `_await_turn_boundary`'s
  `MODEL_RESTART_TURN_TIMEOUT_S` forced swap, and `_arm_restart_timer` being a no-op when
  `autoRestart` is false or no deadline resolves. These need an injectable `sleep` on the loop (as
  `RestartTimer` already has) or `vi.useFakeTimers()`.
- **Restart guards**: `_restart_connection` declining on a generation mismatch or while already
  restarting (the stopped-loop decline is now covered), and the send gate left closed when
  `BidiBeforeConnectionRestartEvent` throws.
- **Generation fencing**: `_wait_for_model_task`'s bounded 2s reap logging rather than forwarding a
  failure.
- **Reader `finally`**: open blocks written back as incomplete when the stream ends (the barge-in
  and stream-failure halves are covered).
- `test_loop_telemetry.py` — none of the 19 loop-level span assertions are ported; the connection
  span lifetime fix is unverified by a test.
- `test_agent.py` loop-dependent tests — none of the 11 listed in the A2 notes are ported.
- No test covers `_runOutputs` abandoning a `receive()` parked on the queue when `_runInputs` fails.
