# CHUNK A1 handoff — BidiAgent helpers + agent class

## Done (commit 5cb0b7921)

- `blocks.ts` — ported from `_blocks.py`. Classes renamed `StreamedTextBlock` /
  `StreamedReasoningBlock` / `StreamedTranscriptBlock` to avoid clashing with the core
  `TextBlock` / `ReasoningBlock` content-block classes in `src/types/messages.ts`.
  TS `Message` is an immutable-ish class (readonly `role`/`content`), so `toMessage()`
  builds a new `Message` reusing the pending message's `trackingId` instead of spreading
  a dict like Python.
- `restart-timer.ts` — ported from `_restart_timer.py`. `resolveDeadlineS` returns
  `undefined` (not `None`). `RestartTimer` uses an `AbortController` per armed cycle in
  place of the Python `asyncio.Task`; the default sleep is `setTimeout` + abort listener;
  `sleep` is still injectable for tests.
- `telemetry.ts` (at `src/bidi/telemetry.ts`, matching `_telemetry.py`'s location at
  `strands/bidi/_telemetry.py`) — ported. Tracer's `_startSpan`/`_endSpan`/
  `_getCommonAttributes`/`_useLatestConventions` are `private` in TS, so the module casts
  the tracer through a local `TracerInternals` interface (same intent as Python's
  underscore access; no edit to `src/telemetry/tracer.ts`). Two gaps vs Python:
  - no `serialize` helper in the TS telemetry module → `JSON.stringify(tools)`.
  - no `Tracer._redact` analog in TS → `gen_ai.system_instructions` is set unredacted.
    FLAG for review.
- `loop.ts` — skeleton only: `AgentLoop` with `start`/`stop`/`send`/`receive`, each
  throwing `new Error('not implemented')`. Next chunk fills it in.

## NOT done

1. **`agent/agent.ts`** — `BidiAgentConfig` + `class BidiAgent implements LocalAgent`
   per `api-design.md` §5. Not started. Python source: `strands-py/src/strands/bidi/agent/agent.py`
   (ctor lines 95–215, properties 217–300, `start`/`send`/`receive`/`stop` 337–463,
   `take_snapshot`/`load_snapshot` 465–544, `__aenter__`/`__aexit__` 545–581,
   `run` 582–660, `_append_messages`/`_update_message` 662–701).
   `send()` normalisation table is at agent.py:399–427 — note the Python error types:
   `RuntimeError("agent not started | call start before sending")`,
   `ValueError("invalid input | expected one text or image key")`,
   `ValueError("invalid input | input list cannot be empty")`,
   `TypeError("invalid input | expected a string, TextBlock, or ImageBlock")`.
2. **`types/io.ts`** — still types `start(agent: LocalAgent)`; must be narrowed to
   `BidiAgent` (import type) once `agent.ts` exists.
3. **`src/bidi/index.ts`** — does not yet export `./agent/index.js` (BidiAgent,
   BidiAgentConfig).
4. **All tests.** Nothing under `src/bidi/agent/__tests__/` yet. To port:
   - `tests/strands/bidi/agent/test_restart_timer.py` → `restart-timer.test.ts`
   - pure-helper tests from `tests/strands/bidi/agent/test_loop_telemetry.py` → `telemetry.test.ts`
   - `tests/strands/bidi/agent/test_agent.py` + `test_snapshot.py` → `agent.test.ts`,
     `snapshot.test.ts` (loop-dependent cases → mark DEFERRED)
   - `_blocks.py` has no dedicated Python test file; cover `blocks.ts` directly.
5. **`ToolCaller.create` widening** — not needed yet (no `agent.ts`); if it requires a
   concrete `Agent`, widen its param to the minimal structural type. That is the one
   allowed edit outside `src/bidi`.

## Gates at handoff

`npx tsc --noEmit -p src/tsconfig.json` → clean. eslint/prettier/vitest not yet run.
