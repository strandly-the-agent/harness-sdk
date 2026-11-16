# Phase 9 review: parity validator (c04b26898)

Gates from worktree `review9-parity-validator` (`GATE_OUT=/tmp/review9_parity_gates .rust-port/gates.sh full`): all 15 steps exit 0 (fmt, clippy x2, test all/nodef, doc, msrv, feat_{bedrock,anthropic,openai,mcp,otel}, deny, conformance); all-features: 3231 passed, 0 failed, 28 ignored (`/tmp/review9_parity-validator_gates.txt`). `tests/mcp_stdio.rs` skips in the gate; run with `STRANDS_TEST_NODE`/`STRANDS_TEST_NODE_MODULES` set, 5/5 pass (`/tmp/review9_parity-validator_stdio.txt`).

Case mapping: all 209 TS cases (`client` 126, `client-annotations` 4, `client-progress` 2, `config.test.node` 48, `mcp-router` 28, `mcp-router.test.node` 1; counts match `count-ts-cases.mjs` and PARITY.md) map to a same-named Rust test or to a rename or gap that PARITY.md names. Every renamed target exists. Error and log texts in `config.node.ts`, `mcp-router.ts` and `mcp-tool.ts` all appear verbatim in Rust, except the branches P-251/P-252 drop. Defaults match TS/SDK: 64 list pages (0 = no cap), 60 s request timeout, 300 ms list-changed debounce, 10 router connections, 2147483647 ms timeout cap, elicitation capability `{form, url}`.

## Findings

1. **`strands-rs/strands/src/mcp/client.rs:812-856`: `call_tool_with` skips the output-schema check the TS client gets from the MCP SDK.** TS's direct path calls the SDK `Client.callTool` (`strands-ts/src/mcp/client.ts:1076`). That call checks `structuredContent` against the tool's listed `outputSchema` (`node_modules/@modelcontextprotocol/client/dist/index.mjs:4244-4285`). `rmcp` has no such check. Neither DIVERGENCES.md nor P-239 mentions this. Evidence, the same server in both SDKs (`/tmp/review9_parity-validator_ts_outschema.txt`, `/tmp/review9_parity-validator_repros.txt` `== outschema`):
   - With no `structuredContent`, TS gives status `error` with `Error: Tool weather has an output schema but did not return structured content`. Rust returns `Ok({"content":[{"type":"text","text":"sunny"}]})`.
   - With `{"forecast":42}` against `forecast: string`, TS gives `error` with `Error: Structured content does not match the tool's output schema: data/forecast must be string`. Rust returns `Ok(..)`, and `McpTool` reports success.

   Fix: keep each listed tool's `output_schema` from `list_tools` (the server-side name is already at hand) and, after `call_result`, apply the SDK rules:
   - `structuredContent` missing and `isError` not set: `McpError::protocol(-32600, "Tool <name> has an output schema but did not return structured content", None)`.
   - Present but invalid: `-32602 "Structured content does not match the tool's output schema: <msg>"`.
   - The schema doesn't compile: `-32602 "Tool '<name>' has an invalid outputSchema: <msg>"`.

   Use the crate's existing JSON-schema validator. Add a test with the two cases above. If this is deferred, add a DIVERGENCES row instead.

2. **`strands-rs/strands/src/mcp/client.rs:838-855`: the MRTR loop gives up one round early and discards the last elicitation answer.** The SDK driver does not count the first response as a round. It fulfils up to 10 rounds, resends after each one, and throws only when round 11 would start, so a tool call makes 11 requests and runs the callback 10 times (`dist/src-WCy6ifGf.mjs:5137,5220-5273`). Rust sends 10 requests, runs the elicitation callback 10 times, and drops the 10th answer without sending it. Repro (`/tmp/review9_parity-validator_src_main.rs.txt`, a 2026-07-28 server that always answers `input_required`) prints `tools/call requests sent=10 elicitation callbacks run=10` and `error=input_required did not complete within 10 MRTR rounds`.

   The same loop also differs from the driver in four other ways (read from the code, not run):
   - Each round gets a fresh `max_total_timeout` (`client.rs:867`), so a call with `timeout` can last up to 10x that timeout. The SDK shrinks one budget across all legs and fails with `Maximum total timeout exceeded` (`:5255-5263`).
   - A round with only `requestState` resends at once. The SDK waits 250 ms first (`REQUEST_STATE_ONLY_LEG_PACING_MS`, `:5143,5250`).
   - The error texts the model sees differ from the SDK's: the round limit (`:5166-5167`); a missing handler, which reads `Cannot fulfil input request '<key>': no handler is registered for '<method>' on this client. ...` (`:6492`); an unsupported request (`:6487`); and an empty `input_required`, which reads `Invalid result for tools/call: input_required carries neither inputRequests nor requestState ...` (`:3979`). Rust has `input_required did not complete within 10 MRTR rounds`, `elicitation requested but no elicitation callback is configured`, `input request <key> is not supported` and `Unexpected response type` (`client.rs:853,911,907,902`).
   - The SDK reports each round to `onprogress` as `Fulfilling input required by '<method>' (round N)` (`:5229-5232`). Rust does not.

   Fix:
   - Fulfil and resend up to `MRTR_MAX_ROUNDS` times after the first response, and fail only when the response after the 10th resend still requires input.
   - Track one deadline from the first send and pass the remaining time as `max_total_timeout` to every leg.
   - Sleep 250 ms (cancellable) before a leg that carries only `requestState`.
   - Use the SDK texts.
   - Add tests for an always-`input_required` server (11 requests, 10 callbacks) and for the total timeout across rounds.

3. **`strands-rs/strands/src/mcp/client.rs:914-917`: an elicitation callback during MRTR never sees the call cancelled, and the call keeps waiting for it.** The SDK hands the handler `mcpReq.signal` tied to the call's abort signal (`linkedRoundAbort`, `dist/src-WCy6ifGf.mjs:5198-5208,5242-5244`; `dispatchInputRequest` `:6496-6501`). Rust passes a fresh `CancelSignal` whose handle is dropped unused, and awaits the callback outside the cancel `select!` in `call_once`. Repro (`/tmp/review9_parity-validator_src_mrtrcancel.rs.txt`): the call's signal is cancelled at 200 ms, and the callback waits on `ctx.cancel_signal()` or 3 s. Output: `call ended after 3.003s; callback saw cancel=false; result is_err=true`, then `The operation was aborted`. An agent cancelled while a user-facing elicitation prompt is open therefore hangs until the prompt is answered.

   Fix: in `fulfill_inputs`, take `options.cancel_signal`, build the context from a child signal that is cancelled when the call's signal fires, and race the callback against `signal.cancelled()`, returning `abort_error(signal)`. Add a test.

4. **`strands-rs/docs/PARITY.md:256-258`: three integ specs are marked `live`, but no Rust live test covers them.** The rows are `mcp-connect-abort.test.node.ts`, `mcp-legacy-fallback.test.node.ts` and `mcp-reconnect.test.node.ts`. PARITY.md:234 says `live` rows run as Rust live tests. There is no MCP `live_*` test (`grep -rn "fn live_" strands-rs/strands` finds only `{anthropic,bedrock,mantle,openai}_live.rs`), and these three specs use no model: their only matches for `model` are `@modelcontextprotocol` imports. Their non-task cases are covered offline:
   - `mcp_client::keeps_the_connection_shared_after_its_creator_aborts_continue_on_error_{false,true}`
   - `streamable_http::falls_back_to_the_legacy_protocol_revision_over_{streamable_http,a_pipe}`
   - `connect_retry_transport_rebuild::{connects_a_client_again_after_disconnect,retries_a_caller_supplied_transport_on_the_same_client_and_transport}`
   - `tests/mcp_stdio.rs`

   The rows at :262-263 (`mcp.test.node.ts`, `mcp-v2.test.node.ts`) also have no Rust live counterpart yet. Fix: give the three model-free rows the offline Rust tests that cover them (and a reason for `mcp-reconnect`'s SEP-2663 case). For the two agent specs, either add `strands/tests/mcp_live.rs` (`#[ignore = "live bedrock"]`, the in-process test server plus `Agent`), or note that it lands with the Phase 11 live sweep.

## Decisions

I agree with P-230..P-253 as logged. P-237 departs from section 12's "`requestTimeout`", and the departure is justified: at the pin, `requestTimeout` exists only in `TasksConfig` (`strands-ts/src/mcp/client.ts:164-182`). P-239 should mention that `call_tool` returns the result without the SDK's output-schema check, or finding 1 should be fixed. P-249's "drives up to 10 rounds" doesn't count rounds the way the SDK does (finding 2).

Verdict: changes requested. Gates and case mapping are clean, but findings 1-3 are behaviour divergences in the MCP tool-call path that change what the model sees, and finding 4 is a PARITY.md accuracy fix.
