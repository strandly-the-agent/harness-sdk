# CHUNK A1 handoff — what the loop chunk still needs

`agent/agent.ts` is complete (`BidiAgentConfig` + `BidiAgent implements LocalAgent`), with
`__tests__/agent.test.ts` (43) and `__tests__/snapshot.test.ts` (21) passing. Remaining work
belongs to the loop chunk:

1. **`loop.ts`** is still a skeleton (`start`/`stop`/`send`/`receive` throw). It now takes the
   owning agent: `new AgentLoop(agent)`. The agent exposes internals the loop needs:
   `_appendMessages(...messages)` and `_updateMessage(message, { strict })` (promise-chain
   message lock, `MessageAddedEvent` fired per message).
2. **Model resolution** — the constructor throws `'model must be a BidiModel, string, or
   undefined'` for anything that is not a `BidiModel`. Once `bidi/models/bedrock.ts` lands,
   wire string → `new BedrockNovaSonicModel({ modelId })` and undefined →
   `modelId: 'amazon.nova-2-5-sonic'` (see the `TODO(port)` in the constructor) and add the two
   `test_bidi_agent_init_with_default_model` / `_with_model_id` tests.
3. **Loop-dependent Python tests not ported** (they assert on `agent.model.send` /
   `model.start` kwargs / history written by the loop, which the mocked loop cannot produce):
   `test_bidi_agent_tool_emits_shared_hook_events_and_retries`,
   `test_bidi_agent_tool_injects_local_agent`, `test_bidi_agent_tool_context_receives_cancel_signal`,
   `test_bidi_agent_tool_integration`, `test_run_cancel_cleans_up_and_allows_reuse`,
   `test_send_normalizes_media` (history assertions), `test_send_preserves_input_identity`,
   `test_send_concurrent_text`, `test_send_list_preserves_order_and_history` (history half),
   `test_bidi_agent_receive_events_from_model`, `test_bidi_agent_start_receive_propagates_model_errors`,
   `test_bidi_agent_state_consistency`, `test_update_message_finds_copied_message_after_history_edit`,
   `test_update_message_when_message_removed`,
   `test_load_snapshot_history_is_sent_on_next_start`.
4. **`telemetry.test.ts`** — pure-helper tests from `test_loop_telemetry.py` are still unported;
   `blocks.ts` has no tests (no Python test file either).
