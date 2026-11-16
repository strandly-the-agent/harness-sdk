# Live test results

Owner: the Phase 11 live sweep (track `p11live`). Results below are from 2026-11-16 unless a line says otherwise. No secrets, account ids or response bodies are recorded here.

## How the sweep was run

```bash
STRANDS_TEST_NODE=<node binary> STRANDS_TEST_NODE_MODULES=<a node_modules with @modelcontextprotocol/sdk, @modelcontextprotocol/server and zod> \
  cargo test -p strands --all-features -- --ignored live_ --nocapture > /tmp/rs_live_p11.txt 2>&1; echo "EXIT: $?"
```

AWS credentials came from the default chain and were used for Bedrock and Bedrock Mantle model calls only; the tests create, change and delete no AWS resource. `ANTHROPIC_API_KEY` and `OPENAI_API_KEY` were not set. Every output file was scanned with the section 9.5 patterns and the environment-value checks before it was read: all clean.

| Run | Commit | `live_` tests | Ran and passed | Skipped (key missing) | Failed | Exit |
|---|---|---|---|---|---|---|
| 1 | `5dbfffe94` | 66 | 53 | 13 | 0 | 0 |
| 2 | `5dbfffe94` | 66 | 53 | 13 | 0 | 0 |
| 3 | `d53f7a521` | 67 | 54 | 13 | 0 | 0 |

Run 3 adds `live_handles_multiple_media_blocks_in_single_request`. Commits after `d53f7a521` change documentation, a doc comment, and three lint fixes with no change in behaviour (an `allow` on the skip message, backticks in a doc comment, and a copy in place of a `clone` in `live_telemetry.rs`); `live_telemetry.rs` was rerun after its fix: 2 ran, 2 passed.

A skipped test returns early and the test harness prints `ok` for it. The 13 skips were counted from the `skipping:` lines (`--nocapture`): 5 `skipping: ANTHROPIC_API_KEY is not set` and 8 `skipping: OPENAI_API_KEY is not set` in each run, and no skip line from any Bedrock, Mantle or MCP test. No request was sent to Anthropic or OpenAI.

## Known wording-dependent failure

`live_context.rs::live_summarizes_the_tool_call_in_a_single_summarizer_request` failed in 2 of 35 runs (26 runs of the test alone or with one other test: 0 failures; 9 runs of the whole file in parallel, three of them the sweeps above: 2 failures). Both failures were on its last assertion, `assert!(summaries[0].contains("FX-7731"), ..)`. The panic message of the captured failure starts

```
summary: [Summarized: 2 messages, ~29 tokens]
```

and continues with the model's summary text (not reproduced here), which describes the `lookup_order` tool call as one the model should not have made and leaves out the tool result, so it lacks the tracking number `FX-7731` the assertion looks for. In both failing runs the assertions before it held: the summarizer received exactly one request, that request contained the tool use and the tool result with `FX-7731`, and the summary header was `[Summarized: 2 messages`. The request has the same shape as the TS one at the pin: the same guard and instruction text, and a `noop` tool spec injected because the messages hold tool blocks and no tool specs are given (TS `models/bedrock.ts:967-976`, Rust `models/bedrock/format.rs:329`). The TS test was not run in this environment, so whether TS shows the same rate is not known. The request content does not depend on how many tests run in parallel; the split between isolated and whole-file runs is reported as measured and not explained.

## Test bugs fixed before the sweeps

These were errors in the new tests, found on their first runs and corrected; none was an SDK defect.

- `live_emits_reasoning_content_with_thinking_model` looked for the block under `reasoningContent`; the wire key is `reasoning`.
- `live_denies_all_tools_when_no_principal_identity_fail_closed` used the prompt `Search for something`, to which the model answered without calling the tool; the prompt now names a query.
- `live_agentic_truncate_context_drops_messages_and_pin_protects_them` looked for a tool named `pin`; the tool is `pin_context`.
- `live_telemetry_records_the_span_hierarchy_attributes_and_usage_of_a_tool_invocation` filtered on a cycle span named `execute_event_loop_cycle`; the SDK and the TS spec use `execute_agent_loop_cycle`.

No SDK bug was found by this sweep, and nothing under `src/` was changed.

## Not covered

- `models/openai/mantle-routing.test.node.ts`: no Rust live test (the base-path resolver and the token minting are crate-private; see P-560 in the staging decisions).
- `models/bedrock.test.node.ts` (string model id, region from `AWS_REGION`, explicit region precedence): no Rust live test.
- `models/openai/session-cache-key.test.ts` and the stateful-model case of `session-manager.test.node.ts`: no Rust live test.
- `agent.test.ts`: PDF and document input, video, citations, `Message[]` input.
- `agent.cancel.test.ts`: the timer and `AbortSignal.timeout()` cases.
- `interventions.test.ts`: the cases listed in the staged parity row; only the Bedrock provider.
- `mcp/*.test.node.ts`: the Streamable HTTP transport variants (stdio only), and the filtering and prefixing cases against the v2 server.
- `telemetry.test.node.ts`: the `getTracer`, `register()` and `Metrics Integration` cases (their Rust API is being built by another track), span events and timing attributes, tool-error and mid-loop error spans, `stream()`, concurrent agents.
- `context-manager-summarize-tool-calls.test.ts`: the OpenAI and Anthropic cases.
- Bedrock API-key (bearer) authentication (no Bedrock API key in this environment), guardrails (need a guardrail resource; creating one is not allowed), audio.
- Everything in `anthropic_live.rs` and `openai_live.rs` (keys missing).

Deviations from the TS specs in the tests that exist: `http_request` fetches from a localhost server the test starts, where TS calls Open-Meteo; `SummarizingConversationManager::reduce` and the summarize strategy are reached through an invocation, where TS calls them directly; MCP uses plain-JavaScript ports of the TS fixture servers.

## New in this sweep


### `strands/tests/live_agent.rs`

TS specs: `agent.test.ts`, `agent.cancel.test.ts`, `agent-as-tool.test.ts`, `interrupt.test.ts`. Region `us-west-2`. Models: `us.anthropic.claude-haiku-4-5-20251001-v1:0`; the reasoning test uses `global.anthropic.claude-sonnet-4-6` with `thinking.budget_tokens: 1024`.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_cancels_before_tool_execution` | pass | pass | pass | cancel at the `toolUse` model message: the tool body never runs, the history holds the tool use and an `error` tool result |
| `live_cancels_during_model_streaming_and_allows_reuse_after_cancellation` | pass | pass | pass | cancel on the first model stream update gives stop reason `cancelled` and an assistant last message; the next invocation ends `endTurn` |
| `live_emits_reasoning_content_with_thinking_model` | pass | pass | pass | `reasoningContentDelta` events; the assistant message keeps a signed reasoning block; a second turn with that block in the history ends `endTurn` |
| `live_handles_invocation_streaming_system_prompts_and_tool_use` | pass | pass | pass | stream events include `beforeInvocationEvent` and a `modelMetadataEvent` with input/output tokens and `latencyMs` > 0; `calculator` attempted and succeeded with result `Result: 56088`; `endTurn`; cycle count >= 2; a second turn appends to the history |
| `live_handles_multiple_media_blocks_in_single_request` | not in this run | not in this run | pass | a text document block and a PNG image block in one `ContentBlock` list; the answer names the animal from the document and the colour of the image (a wording check: there is no tool in this exchange); `endTurn` |
| `live_interrupts_before_tool_call_resumes_with_approval` | pass | pass | pass | hook interrupt `test_interrupt` with reason `need approval`; resume with `APPROVE` runs `time_tool` (`12:00`); `endTurn` |
| `live_interrupts_before_tool_call_resumes_with_rejection_cancels_tool` | pass | pass | pass | resume with `REJECT`: an `error` tool result in a user message, `time_tool` never succeeds; `endTurn` |
| `live_interrupts_from_tool_callback_resumes_with_response` | pass | pass | pass | tool interrupt with no response set; resume with `12:01`, which becomes the tool result; `endTurn` |
| `live_maintains_message_history_and_conversation_context` | pass | pass | pass | two turns give roles user, assistant, user, assistant; the second answer names Alice |
| `live_parent_agent_invokes_a_sub_agent_tool_that_uses_a_standard_tool_and_gets_a_result` | pass | pass | pass | tool metric `tiger_expert` success count >= 1; the sub-agent tool result is `success`; `endTurn` |

### `strands/tests/live_interventions.rs`

TS specs: `interventions.test.ts`, `cedar.test.node.ts`. Region `us-west-2`. Models: `us.anthropic.claude-haiku-4-5-20251001-v1:0`.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_allows_tool_execution_when_policy_permits` | pass | pass | pass | Cedar `test.cedar`, principal `alice`: `search` succeeds with `Results for:` |
| `live_confirm_pauses_agent_execution_and_resumes_with_approval_or_denial` | pass | pass | pass | through `stream()`: stop reason `interrupt`, interrupt name `confirm-tool`, reason `Approve use of time_tool?`; `"  yes  "` runs the tool, `false` leaves an `error` result |
| `live_denies_all_tools_when_no_principal_identity_fail_closed` | pass | pass | pass | `permit-all.cedar` with no principal: `search` attempted, result `DENIED` |
| `live_denies_tool_execution_when_policy_forbids` | pass | pass | pass | `permit-search-deny-delete.cedar`: `delete_record` attempted, never succeeds, result `DENIED: Access denied by Cedar policy` |
| `live_deny_blocks_only_the_specified_tool_and_transform_modifies_tool_input` | pass | pass | pass | `time_tool` results are all `error` with `DENIED: Tool "time_tool" is not allowed`; `weather_tool` succeeds; `echo_tool` result starts `Echo: [TRANSFORMED]`; the second handler sees every call except the denied ones |
| `live_deny_on_before_invocation_and_before_model_call_prevents_the_model_call` | pass | pass | pass | last message text contains `DENIED`, zero output tokens, no tool results (no model call is made) |
| `live_guides_on_model_calls_inject_feedback_and_after_tool_transform_redacts_output` | pass | pass | pass | both guidance texts appear as user messages, at least 3 model calls, the tool result reads `[REDACTED]:[REDACTED]` and never `12:00` |
| `live_on_error_policies_throw_proceed_and_deny` | pass | pass | pass | `throw`: the invocation fails with `Handler exploded`; `proceed`: the tool runs and the later handler is called; `deny`: result `DENIED: Handler threw: Handler exploded`, later handler not called |
| `live_preemptive_confirm_does_not_pause_and_guides_accumulate_into_one_cancellation` | pass | pass | pass | a preemptive answer with a custom `evaluate` approves without an interrupt; two guide handlers give one result `GUIDANCE: [guide-one] ..\n[guide-two] ..` |

### `strands/tests/live_context.rs`

TS specs: `conversation-manager/*.test.ts` (4 files). Region `us-west-2`. Models: `us.anthropic.claude-haiku-4-5-20251001-v1:0`; the reduction test also runs `global.anthropic.claude-sonnet-4-6` with thinking.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_agentic_context_status_is_injected_and_token_usage_grows_across_turns` | pass | pass | pass | a request ends with a `<context-status>` note holding `<used>` and `<remaining>`, the note is not stored in the history; `calculator` result `42`; input tokens in message metadata grow across turns; a non-agentic agent sends no note |
| `live_agentic_summarize_context_replaces_older_messages_and_agent_remains_coherent` | pass | pass | pass | `summarize_context` is called, the history shrinks, `Grass is green.` is no longer a verbatim user message, the next answer names Alice |
| `live_agentic_truncate_context_drops_messages_and_pin_protects_them` | pass | pass | pass | `pin_context` succeeds, `truncate_context` is called, the history shrinks, the pinned `ZEBRA-9` message stays and a filler message is gone |
| `live_drop_truncate_and_summarize_keep_message_0_verbatim_and_every_reduced_history_valid` | pass | pass | pass | drop, truncate and summarize with `contextWindowLimit: 400`, four tool turns each, on both models: every turn ends `endTurn` (so Bedrock accepted every reduced history), message 0 is the pinned task verbatim, fewer than 10 messages |
| `live_keeps_tool_use_result_pairs_balanced_after_summarization` | pass | pass | pass | in the reduced request every tool result id has its tool use and `time-1` is gone; `calculator` then returns `Result: 43` |
| `live_summarizes_older_messages_and_agent_remains_functional_after_summarization` | pass | pass | pass | three model requests (injected overflow, summarizer, retry); the retried request is shorter; message 0 is a user summary over 50 characters; `endTurn` |
| `live_summarizes_the_tool_call_in_a_single_summarizer_request` | pass | pass | pass | exactly one summarizer request, holding the `lookup_order` tool use and its result; one `[Summarized: 2 messages` block; no tool blocks left; the summary names `FX-7731` (wording-dependent, see below) |

### `strands/tests/live_mcp.rs`

TS specs: `mcp/mcp.test.node.ts`, `mcp/mcp-v2.test.node.ts`. Region `us-west-2`. Models: `us.anthropic.claude-haiku-4-5-20251001-v1:0`.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_mcp_agent_can_use_mcp_tool_that_requests_elicitation` | pass | pass | pass | the elicitation callback is called; `confirm_action` succeeds with `confirmed by user` |
| `live_mcp_agent_can_use_multiple_mcp_tools_and_handles_tool_errors` | pass | pass | pass | `echo` result `Multi-turn test`; `calculator` called with `operation: add`, result `Result: 42`; `error_tool` attempted with an `error` result; `endTurn` |
| `live_mcp_filtering_prefixing_prompts_and_resources` | pass | pass | pass | no model call: prefixed and filtered tool names, one client shared by two agents, two prefixes, direct tool calls, prompt listing and rendering, static and templated resources |
| `live_mcp_v2_agent_can_use_mcp_tool_that_requests_elicitation` | pass | pass | pass | input-required elicitation: the callback is called and `confirm_action` succeeds with `confirmed by user` |
| `live_mcp_v2_negotiates_protocol_revision_2026_07_28_and_agent_uses_its_tools` | pass | pass | pass | negotiated protocol version `2026-07-28`; `echo`, `calculator` (`Result: 42`) and an `error` result from `error_tool` |

### `strands/tests/live_routing.rs`

TS specs: `models/classifier-strategy.test.ts`. Region `us-west-2`. Models: candidates `us.anthropic.claude-haiku-4-5-20251001-v1:0`, `us.amazon.nova-pro-v1:0`, `us.amazon.nova-lite-v1:0`; classifier `us.anthropic.claude-haiku-4-5-20251001-v1:0` at temperature 0.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_classifies_and_serves_a_request_on_the_expected_candidate` | pass | pass | pass | the first model call is served by the `advanced model` candidate (`us.amazon.nova-pro-v1:0`); `endTurn`; non-empty answer; usage > 0 |

### `strands/tests/live_telemetry.rs`

TS specs: `telemetry.test.node.ts`. Region `us-west-2`. Models: `us.anthropic.claude-haiku-4-5-20251001-v1:0`.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_telemetry_records_error_status_when_the_model_call_fails` | pass | pass | pass | an unknown model id: error status on the agent span and the `chat` span, an `exception` event on the `chat` span |
| `live_telemetry_records_the_span_hierarchy_attributes_and_usage_of_a_tool_invocation` | pass | pass | pass | one `invoke_agent telemetry_agent` root with agent name, model id and the custom trace attribute; two `execute_agent_loop_cycle` spans under it; one `chat` span per cycle with model id and usage; agent-span usage equals the sum over the `chat` spans and the `AgentResult` usage; `execute_tool time_tool` under a cycle with tool name and call id; status OK on all |

### `strands/tests/live_tools.rs`

TS specs: `tools/bash.test.node.ts`, `tools/http-request.test.ts`, `tools/notebook.test.ts`, `skills/agent-skills.test.node.ts`. Region `us-west-2`. Models: `us.anthropic.claude-haiku-4-5-20251001-v1:0`.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_agent_uses_http_request_tool_to_fetch_a_forecast` | pass | pass | pass | `http_request` called with method GET and the exact localhost URL; the tool result holds the served JSON marker; `endTurn` |
| `live_bash_captures_stdout_and_stderr_and_handles_command_errors` | pass | pass | pass | `bash` called with a `command` containing the text; results contain `Hello from bash`, the stderr marker and the variable value; the unknown command gives a result with `not found` or exit code 127 |
| `live_notebook_persists_state_across_invocations_and_agent_instances` | pass | pass | pass | app state `notebooks.test` holds the title, then the added item; a second agent built from the saved state reads it through the tool; reading a missing notebook adds a tool result and no state entry |
| `live_skills_activates_a_skill_tracks_it_in_app_state_and_replaces_the_skills_block` | pass | pass | pass | `skills` called with `skill_name: summarization`; the tool result and the answer hold the codeword; the system prompt lists both skills; activated skills in app state grow to two; after `set_available_skills` the prompt has one skills block without the removed skill |
| `live_skills_loads_a_skill_from_disk_and_activates_it` | pass | pass | pass | one available skill `code-review` loaded from a directory; the `skills` result and the answer hold the codeword; app state lists it |

## Provider tests (unchanged files, rerun in the three sweeps)

This sweep reran these files and did not compare them case by case with their TS specs.

### Bedrock (`strands/tests/bedrock_live.rs`)

Region `us-west-2`, model `us.anthropic.claude-haiku-4-5-20251001-v1:0`. TS spec: `models/bedrock.test.ts`.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_cache_hit_with_tools_and_messages` | pass | pass | pass | `cacheConfig { strategy: auto }` with a tool spec and a large unique message: first call `cacheWriteInputTokens > 0`, second identical call `cacheReadInputTokens > 0` (tools and message cache points) |
| `live_invalid_model_id_fails_with_a_model_error` | pass | pass | pass | an unknown model id fails with a `ModelError`-family error |
| `live_streaming_events_and_max_tokens` | pass | pass | pass | ConverseStream: first event `messageStart`, text deltas, `endTurn`; `max_tokens: 5` gives `maxTokens` |
| `live_structured_output` | pass | pass | pass | `structured_output_schema::<Person>()` parses `{ name: "Ada", age: 36 }` |
| `live_text_with_the_converse_api` | pass | pass | pass | `stream: false` (Converse): stop reason `endTurn`, a non-empty text delta, input and output tokens > 0 |
| `live_tool_round_trip_through_an_agent` | pass | pass | pass | Agent with a `#[tool]`: `get_weather` called with `city` Seattle, tool result `success` containing the tool's output, final stop `endTurn` |

### Bedrock Mantle (`strands/tests/mantle_live.rs`)

Region `us-east-1`, model `openai.gpt-oss-120b` (as in `strands-ts/test/integ/models/openai/mantle.test.node.ts`); the cache test uses `openai.gpt-6.1-sol`. Default AWS credential chain, bearer token minted by the Rust SigV4 port.

| Test | Run 1 | Run 2 | Run 3 | Checks |
|---|---|---|---|---|
| `live_mantle_cache_hit` | pass | pass | pass | second identical request (`cache_config` set, session key `strands-rust-live-cache`) reports `cacheReadInputTokens > 0` |
| `live_mantle_reasoning_multi_turn` | pass | pass | pass | `params.reasoning.effort = low`, two turns, second ends `endTurn` |
| `live_mantle_stateful_conversation` | pass | pass | pass | `stateful: true`, second turn answers with the name from the first (`previous_response_id` chaining) |
| `live_mantle_streaming` | pass | pass | pass | first event message start, text deltas, usage with input and output tokens |
| `live_mantle_structured_output` | pass | pass | pass | `Person { name: "John..", age: 30 }` through the structured output tool |
| `live_mantle_text_chat` | pass | pass | pass | Chat Completions agent invoke ends `endTurn`, reply contains `4` |
| `live_mantle_text_responses` | pass | pass | pass | same on the Responses API |
| `live_mantle_tool_round_trip_chat` | pass | pass | pass | `get_weather` called with `city` containing Lisbon, result contains `21 degrees`, `endTurn` |
| `live_mantle_tool_round_trip_responses` | pass | pass | pass | same on the Responses API |

### Anthropic (`strands/tests/anthropic_live.rs`)

`ANTHROPIC_API_KEY` was not set: each of the 5 tests printed `skipping: ANTHROPIC_API_KEY is not set` and returned, in all three runs. Model in the tests: `claude-haiku-4-5`.

| Test | Result | Checks |
|---|---|---|
| `live_cache_hit` | skipped (no key) | a system prompt cache point over ~8k tokens: the first call reports cache writes, the second cache reads |
| `live_streaming` | skipped (no key) | `stream` starts with a message start, has text deltas, `maxTokens` stop at `max_tokens: 20`, output tokens <= 20 |
| `live_structured_output` | skipped (no key) | `structured_output::<Person>()` parses with `age == 30` |
| `live_text` | skipped (no key) | agent invoke ends with `endTurn` and a non-empty text block |
| `live_tool_round_trip` | skipped (no key) | the model calls `get_weather` with `city` containing Lisbon, the tool result holds the tool output, the turn ends with `endTurn` |

### OpenAI (`strands/tests/openai_live.rs`)

`OPENAI_API_KEY` was not set: each of the 8 tests printed `skipping: OPENAI_API_KEY is not set` and returned, in all three runs.

| Test | Result |
|---|---|
| `live_cache_hit` | skipped (no key) |
| `live_stateful_responses` | skipped (no key) |
| `live_streaming` | skipped (no key) |
| `live_structured_output` | skipped (no key) |
| `live_text_chat` | skipped (no key) |
| `live_text_responses` | skipped (no key) |
| `live_tool_round_trip_chat` | skipped (no key) |
| `live_tool_round_trip_responses` | skipped (no key) |

## Earlier records kept from previous sessions

- MCP offline stdio fixture, 2026-11-15 (not rerun today): `STRANDS_TEST_NODE=<node> STRANDS_TEST_NODE_MODULES=<repo node_modules> cargo test -p strands --features mcp --test mcp_stdio`: 5 passed, 0 failed.
- Sandbox and vended tools (Phase 8): the sandbox, `web_fetch` and `subagent` have no live tests; they are covered by unit tests against the host, child processes, mock models and a local HTTP server. `bash`, `http_request` and `notebook` now have the live tests above.
