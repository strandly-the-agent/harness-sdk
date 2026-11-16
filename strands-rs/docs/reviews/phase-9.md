# Phase 9 review: MCP

Three independent reviewers (correctness, API bar-raiser, parity validator) reviewed the MCP paths (`mcp*`, `vended_tools/mcp_router*`, the MCP tests and the MCP wiring) at `c04b26898`. The parity validator ran `gates.sh full` there, which passed.

| finding | disposition | commit |
|---|---|---|
| correctness-1 | Fixed: the agent's `on_tools_changed` callback holds the change queue weakly (P-254). Regression test `mcp::tests::robustness::dropping_an_agent_after_a_tools_refresh_closes_the_connection` (fails without the fix: `condition never held`) | 43902cb3d |
| correctness-2 | Fixed: the generation is read before the connect task is spawned and `Connected`/`Failed` are stored under the `state` lock `disconnect` bumps the generation in (P-254). Test `mcp::tests::robustness::a_disconnect_right_after_connect_starts_leaves_the_client_disconnected` | 43902cb3d |
| correctness-3 | Fixed: `disconnect` invalidates pending tools-changed tickets and a refresh lists over the live peer only (P-254). Test `mcp::tests::robustness::a_pending_tools_refresh_does_not_reconnect_after_disconnect` | 43902cb3d |
| correctness-4 | Fixed: the callback runs under `catch_unwind` and logs the TS text; a drop guard clears `running`; the final `pending` check and reset share one critical section (P-254). Test `mcp::tests::robustness::a_panicking_tools_changed_callback_leaves_later_refreshes_working` | 43902cb3d |
| correctness-5 | Fixed: progress is delivered inline on the transport's receive path (`ProgressTap`), under the routes lock (P-255). Test `mcp::tests::robustness::progress_before_the_result_reaches_the_callback_in_order` (4-worker runtime, 30 calls x 50 notifications) | 43902cb3d |
| correctness-6 | Fixed with parity-2/3: the elicitation signal is cancelled with the call's, the round is raced against it, and one deadline bounds all rounds (`Maximum total timeout exceeded`) (P-256). Tests `mcp::tests::robustness::cancelling_a_call_cancels_its_pending_input_required_elicitation`, `the_call_timeout_bounds_every_input_required_round` | 43902cb3d |
| correctness-7 | Fixed: with `auth`/`auth_provider`, a user `Authorization` header is dropped (P-257). Test `mcp::tests::robustness::the_auth_token_replaces_a_user_authorization_header` | 43902cb3d |
| correctness-8 | Fixed: `ClientCredentialsClient` runs the grant again when no token is usable or on a 401 and retries once (P-257). Test `mcp::tests::robustness::an_expired_client_credentials_token_is_granted_again` | 43902cb3d |
| correctness-9 | Fixed: prompt and resource list methods return an empty result when the server lacks the capability (P-258, amends P-248). Test `mcp::tests::robustness::lists_no_prompts_or_resources_from_a_server_without_those_capabilities` | 43902cb3d |
| parity-2 | Fixed: SDK driver (11 requests, 10 fulfilments), shrinking total timeout, 250 ms `requestState`-only pacing, SDK texts, round progress (P-256). Tests `mcp::tests::robustness::fulfils_ten_input_required_rounds_before_giving_up`, `paces_a_request_state_only_round`, `names_the_missing_handler_for_an_input_request`, `the_call_timeout_bounds_every_input_required_round` | 43902cb3d |
| parity-3 | Fixed with correctness-6 (P-256). Test `mcp::tests::robustness::cancelling_a_call_cancels_its_pending_input_required_elicitation` | 43902cb3d |
| parity-1 | Fixed: `call_tool_with` checks the result against the tool's listed `outputSchema` with the SDK rules and texts (P-256; divergence row for the per-error wording). Test `mcp::tests::robustness::checks_structured_content_against_the_tool_output_schema` (fails without the fix: the missing and invalid cases return `Ok`) | e1e62c87f |
| api-1 | Fixed: `pub use regex;` and `pub use reqwest;` in `strands::mcp` (P-259) | 94fe56ca7 |
| api-2 | Fixed: `McpServerCredentials` removed; `McpClientCredentials` is serde-enabled and `McpServerConfig::auth` uses it (P-259). Test `mcp::tests::robustness::a_config_entry_reads_auth_as_client_credentials` | 94fe56ca7 |
| api-3 | Fixed: `bon::Builder` on `McpClientCredentials`, `McpToolFilters`, `SerializableMcpToolFilters`, `McpServerConfig`, `McpLoadServersOptions` (the sixth, `McpServerCredentials`, is removed by api-2) (P-259) | 94fe56ca7 |
| api-4 | Fixed: `McpServerConfig::tasks_config` removed; the raw entry's `tasksConfig` is rejected before parsing with the same text; existing tests in `mcp::tests::config` still pass (P-259) | 94fe56ca7 |
| api-5 | Fixed: `list_prompts()`, `list_resources()`, `list_resource_templates()` take no arguments; `*_with(McpListOptions)` take pagination (P-259) | 2977de098 |
| api-6 | Fixed: `connect_with(reconnect, McpConnectOptions)` with `McpConnectOptions { cancel_signal }` (config struct, `Default`, builder; re-exported) (P-259) | 2977de098 |
| api-7 | Logged rather than hidden: `client_version`, `negotiated_protocol_version` and `McpTool::server_name` kept as small accessors, each with its reason (P-259 item 7) | 2977de098 |
| api-8 | Fixed: the P-233 cross-reference now cites P-241 | integration commit |
| parity-4 | Fixed: the three model-free rows name their offline Rust tests (and the SEP-2663 case as out of scope); `mcp.test.node.ts` and `mcp-v2.test.node.ts` note that their live tests land with the Phase 11 live sweep (`.rust-port/staging/phase9fix/parity.md`) | 2977de098 |
