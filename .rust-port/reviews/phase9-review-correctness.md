# Phase 9 correctness review (`c04b26898`)

I found nine verified problems. Eight of them reproduce with a failing test. The existing MCP suite passes: `cargo test -p strands --no-default-features --features mcp --lib mcp` gives 227 passed. The repros are throwaway tests in a scratch copy of the tree (`/tmp/review9c-scratch`, file saved as `/tmp/review9_correctness_repros.rs`). Their output is in `/tmp/review9_correctness_repro{1..5}.txt`. Line numbers refer to `c04b26898`.

1. **`strands-rs/strands/src/mcp/agent_tools.rs:34-41` (with `mcp_tool.rs:28`, `client.rs:297`).** An agent dropped after a tools-list refresh leaks the connection, and with it the stdio child.
   - **What's wrong:** the callback stored in `Inner.on_tools_changed` captures `changes`, which queues `Vec<McpTool>`. Each `McpTool` holds a strong `McpClient`, so a strong cycle runs `Inner → callback → changes → McpTool → Inner`. Only the next `BeforeModelCallEvent` drains the queue and breaks it (`:43-44`). If the agent is dropped before another model call, the cycle is never broken. This breaks the documented "dropping the last clone closes it" (`client.rs:276-277`).
   - **Evidence:** `a_agent_drop_after_refresh_keeps_connection_open` fails with `condition never held`: the transport is still open after the agent and the client are dropped. The control test without a refresh passes (`/tmp/review9_correctness_repro1.txt`).
   - **Fix:** have the callback capture `Arc::downgrade(&changes)` and push only when it upgrades. The hook keeps the strong `Arc`, so dropping the agent drops the queue.

2. **`strands-rs/strands/src/mcp/client.rs:1008-1031, 1045-1046, 1077-1081`.** `disconnect()` doesn't stop a connect that has just started, so the client ends up connected after `disconnect()` returned.
   - **What's wrong:** `establish` reads the generation inside the spawned task (`:1046`), not when `start_connect` is called. A `disconnect()` that runs between the spawn and the task's first poll is never noticed. The task then stores `Conn::Connected` (`:1081`). The check at `:1077` and the store at `:1081` are also separate lock acquisitions, which leaves a second window.
   - **Evidence:** `f_disconnect_right_after_connect_starts_stays_disconnected` (current-thread runtime, deterministic). `connect` returns `Err("MCP client disconnected")` but `connection_state()` is `Connected` (`repro2.txt`). TS reads the generation synchronously in `_connect` and closes the client when it changed (`strands-ts/src/mcp/client.ts:596, 618-625`).
   - **Fix:** read the generation in `start_connect` before `tokio::spawn` and pass it to `establish`. Do the generation check and the `Connected` store under one `state` lock, and bump the generation in `disconnect` while holding that same lock.

3. **`strands-rs/strands/src/mcp/client.rs:660-675` and `:625-637`.** A pending tools-changed refresh reconnects a client after `disconnect()`.
   - **What's wrong:** the 300 ms debounce task calls `refresh_tools`, which calls `list_tools()`, and `list_tools()` connects lazily. A notification that arrives just before `disconnect()` therefore reopens the connection, or re-spawns the stdio child, about 300 ms later.
   - **Evidence:** `g_pending_tools_changed_refresh_does_not_reconnect_after_disconnect`: after `disconnect()` and 500 ms the state is `Connected` (`repro2.txt`). The MCP SDK clears the list-changed debounce timers on close (`node_modules/@modelcontextprotocol/client/dist/index.mjs:3126-3127`, `_resetConnectionState`, called from `close()`).
   - **Fix:** bump `tools_changed` in `disconnect()` so that pending tickets no longer match. Have the refresh path use the live peer only (skip it when the state isn't `Connected`) instead of `list_tools()`'s lazy connect.

4. **`strands-rs/strands/src/mcp/client.rs:625-657`.** A panicking `on_tools_changed` callback turns off tools refresh for good, and the `running`/`pending` handoff can lose an update.
   - **What's wrong:** `running` is reset only on the normal path (`:656`). A panic in the public `set_on_tools_changed` callback at `:641` unwinds the spawned debounce task and leaves `running = true`. From then on every refresh just sets `pending` and returns. The handoff has a race too: the `pending` check (`:652`) and `running = false` (`:656`) are separate lock acquisitions. A notification that arrives between them sets `pending` and returns, and that refresh is lost.
   - **Evidence:** `e_panicking_tools_changed_callback_disables_refresh`: after one panicking refresh, a later `refresh_tools()` with a working callback never calls it (`left: 0, right: 1`, `repro1.txt`). TS resets the flag in `finally` and logs the throw (`strands-ts/src/mcp/client.ts:761-774`). Progress callbacks already get `catch_unwind` (`client.rs:923-937`); this callback doesn't.
   - **Fix:** run the callback under `catch_unwind` and log `failed to refresh tools after toolsChanged notification`. Reset `running` with a drop guard, and do the final `pending` check and `running = false` in one critical section.

5. **`strands-rs/strands/src/mcp/client.rs:859-891` and `:312-313` (cause: `rmcp-3.10.0/src/service.rs:1743`).** Progress notifications arrive out of order or get lost on a multi-thread runtime.
   - **What's wrong:** `rmcp` handles each notification on its own spawned task, so `on_progress` callbacks run in any order. They can also run after the response has been handed to `call_once`. Once `call_once` has removed the route (`:890`), the late notifications land in the `early` queue and are never delivered. The comment claiming "none is lost" (`:312-313`) is wrong.
   - **Evidence:** `i_progress_is_complete_and_ordered` runs on a 4-worker runtime with 30 calls, each sending 50 progress notifications before its result. 18 of the 30 calls saw reordered or missing progress, for example `(9, 0, 0)`: none of 50 delivered, and `(10, 38, 0)` (`repro4.txt`). TS delivers progress in order before the result.
   - **Fix:** route progress in arrival order before `rmcp`'s service loop sees the message. Do this with a `Transport` adapter in `McpTransport::from_fallible_factory` / `from_connect` whose `receive` dispatches `notifications/progress` to `ProgressRoutes` inline. Responses come later in the same stream, so delivery then happens before `call_once` returns. Alternatively, fix `rmcp` upstream to handle notifications sequentially.

6. **`strands-rs/strands/src/mcp/client.rs:896-920` (with `:838-846`).** On a 2026-07-28 connection, an `input_required` (MRTR) round ignores the call's cancel signal and its timeout.
   - **What's wrong:** the elicitation callback gets a fresh `CancelSignal` that is never cancelled (`:914`), and the `await` at `:915-917` is not raced against `options.cancel_signal`. So an agent cancel, which `McpTool::stream` forwards (`mcp_tool.rs:95-101`), can't interrupt a pending elicitation. Each round also gets a full `call_options(options.timeout)`, so a timeout of T allows up to 10×T plus unbounded callback time.
   - **Evidence:** `d_cancel_during_mrtr_elicitation_returns`: after cancel, the call is still pending 3 s later. `j_call_timeout_bounds_mrtr_rounds`: a 200 ms call timeout lets a 600 ms MRTR exchange succeed (`repro1.txt`, `repro5.txt`). The MCP SDK links the caller's abort signal into each round's handler signal (`linkedRoundAbort`, `client/dist/src-WCy6ifGf.mjs:5198-5208, 5240-5244`) and rejects with `Maximum total timeout exceeded` once `maxTotalTimeout` has passed across legs (`:5255-5260`).
   - **Fix:** cancel the elicitation context's signal when `options.cancel_signal` fires, and `select!` the callback against it, returning `abort_error`. Record a deadline before the first round, then fail with `Maximum total timeout exceeded` when it passes. Pass the remaining time as each round's `max_total_timeout`.

7. **`strands-rs/strands/src/mcp/client.rs:1144-1160`.** `auth` or `auth_provider` together with a user `Authorization` header sends two `Authorization` headers.
   - **What's wrong:** `rmcp` sets the bearer token with `bearer_auth` and then appends the custom headers. `reqwest`'s `header()` appends rather than replaces (`reqwest-0.13.5/src/async_impl/request.rs:226`).
   - **Evidence:** `b_auth_and_authorization_header_sends_two_headers` records `["Bearer tok-123", "Bearer user-header"]` on each MCP request (`repro1.txt`). TS builds the headers from `requestInit.headers` and then `headers.set("Authorization", ...)`, so the token wins and only one header is sent (`client/dist/index.mjs:5444-5452`).
   - **Fix:** when `auth` or `auth_provider` is set, drop `authorization` from `custom_headers` in `streamable_http`. The token wins, as in TS.

8. **`strands-rs/strands/src/mcp/client.rs:1162-1208`.** An expired client-credentials token is never fetched again, so a long-lived connection loses auth.
   - **What's wrong:** the grant runs once per connection. Once the token is within 30 s of `expires_in`, `rmcp` tries a refresh. Client-credentials responses normally carry no refresh token, so the refresh returns `AuthorizationRequired`. The request then goes out with no token (`rmcp-3.10.0/src/transport/common/auth/streamable_http_client.rs:39-42`). On the 401 that follows there is no retry, because no token was sent (`:49-51`). Every call after expiry fails until `connect(true)`.
   - **Evidence:** `c_client_credentials_token_is_not_reacquired` (test token endpoint changed to `expires_in: 31`). After 2 s the next request carries no `Authorization` header, and the token endpoint was hit only once (`repro1.txt`). TS's non-interactive provider re-runs the `client_credentials` grant on a 401 and retries the request (`client/dist/index.mjs:5740-5752`, with `auth()` at `:768-787`).
   - **Fix:** wrap the `AuthClient` in a `StreamableHttpClient` that holds the credentials. On `StreamableHttpError::AuthRequired`, or when no token is available, it re-runs `client_credentials(url, credentials)`, swaps in the new client and retries once.

9. **`strands-rs/strands/src/mcp/client.rs:766-787`.** `list_prompts`, `list_resources` and `list_resource_templates` fail against servers that don't advertise those capabilities. TS returns empty lists.
   - **What's wrong:** only `server_tools` checks the capability (`:575-581`). The prompt and resource methods always send the request, and a tools-only server answers `-32601`.
   - **Evidence:** `h_list_prompts_on_a_tools_only_server_is_empty` (the server advertises tools only and answers `prompts/list` with method-not-found) returns `Err(Protocol { code: -32601 })` (`repro3.txt`). The MCP SDK returns `{ prompts: [] }` and `{ resources: [] }` when the capability is missing (`client/dist/index.mjs:3681-3684, 3719-3722`). I disagree with P-248, which records this rule for `listTools` only.
   - **Fix:** in `list_paged`, return an empty result when `server_info()` lacks `prompts` or `resources` respectively (templates fall under `resources`). Update P-248.

**Checked and found correct:**
- `continue_on_error` and the `failed` state, `connect(true)` from `Failed`, and abort-while-waiting.
- No guard is held across an `.await`, and concurrent calls share one connection.
- Pagination and `list_max_pages` match `_listAllPages`: repeated-page stop, `0` means no cap, same error text.
- Trace `_meta` merge, filter semantics (sticky regex, `rejected` wins), and the request timeout and reset-on-progress options.
- SSE: endpoint origin check and the non-200 and POST error texts.
- Auto version fallback is `rmcp`'s `ClientLifecycleMode::Auto`, with legacy `2025-11-25` (`model.rs:201`).
- Stdio env is the default environment merged with `env` (matches `client/dist/stdio.mjs:75-78`), and the child gets graceful shutdown on `disconnect`.
- `load_servers`: `${VAR}`/`${env:VAR}` through the lookup seam, `disabled`, `cwd`, per-entry `continueOnError`, and the non-object error.
- `mcp_router`: per-agent cache, cap and duplicate checks after the awaits, `tool_filters` enforced through the filtered listing.
- No `unwrap`, `expect` or `panic!` in library code.

Verdict: changes requested. Items 1-3 leak live connections or child processes, and items 5, 6 and 8 break progress, cancellation/timeout and auth.
