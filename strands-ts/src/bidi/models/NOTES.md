# NOTES — chunk G1 (`GoogleGeminiLiveModel`)

Implementation is complete and ported in full (`src/bidi/models/google.ts`, exported from
`src/bidi/models/index.ts`). Gates: tsc clean, eslint clean, prettier applied, 63/63 unit tests pass.

## Test coverage gap (next agent)

`strands-py/tests/strands/bidi/models/test_google.py` is 2056 lines / ~90 tests; 63 TS tests cover
the core behaviour (config/audio resolution, connect config, history replay, every event-conversion
path, usage mapping, go-away, throttling, send paths, tool-result shapes, stop, restart/resume) and
the per-turn bracketing table (`describe('turn bracketing')`). Still to port:

- Agent-level restart paths: `test_proactive_restart_end_to_end_through_agent` and the
  GoAway → connection-warning/restart tests (need the bidi agent chunk to land first).
- `test_send_tool_results_preserves_group`, `test_live_tool_uses_structured_parameter_schema`,
  image-block sending, `test_start_passes_merged_config_to_genai` variants.
- `test_update_config_warns_invalid_keys` has no TS analog: there is no `validate_config_keys`
  equivalent in `strands-ts`, so unknown keys are a type error instead of a runtime warning.

# Handoff — `bidi/models/bedrock.ts`

Implementation is complete and green (tsc / eslint / prettier / 48 vitest tests).
What remains is **test coverage**: 48 of the ~73 Python tests in
`strands-py/tests/strands/bidi/models/test_bedrock.py` are ported.

## Not ported (and why)

Intentionally dropped — no TS analogue:

- `test_crt_transport_observes_completed_request_writer`, `test_crt_transport_attaches_request_writer_observer`,
  `test_crt_response_streams_chunks_until_end`, `test_crt_response_read_survives_reader_cancellation`,
  `test_crt_response_reports_cancelled_read_failure` — cover the `_BedrockAWSCRTHTTPClient` /
  `_BedrockAWSCRTHTTPResponse` workaround for `aws-crt-python`; TS uses `NodeHttp2Handler`.
- `test___init__rejects_boto_session_and_region`, `test_valid_region_accepted`, `test_invalid_region_rejected`,
  `test_start_sets_strands_user_agent_on_bedrock_runtime_client` (partly) — no boto session in TS;
  region/user-agent handling is delegated to `BedrockRuntimeClient`.
- `test_update_config_warns_invalid_keys`, `test__init__warns_on_unknown_audio_keys`,
  `test__init__requires_audio_sample_rate` — Python's runtime `validate_config_keys` has no TS analogue
  (the compiler enforces the shape).

Still worth porting:

- `test_restart_uses_updated_config`
- `test_proactive_restart_end_to_end_through_agent` (needs the agent chunk)
- `test_model_stop_after_start_failure`
- `test_send_edge_cases`, `test_send_all_content_types` (remaining permutations)
- `test_response_after_barge_in_finishes_before_next_user_transcript`,
  `test_barge_in_after_response_stop_only_stops_playback`,
  `test_user_content_without_transcript_text_completes`,
  `test_final_assistant_blocks_do_not_change_pending_transcript`,
  `test_user_transcript_stops_before_assistant_audio`,
  `test_user_transcript_closes_response_after_tool_use`,
  `test_tool_calls_keep_response_open_until_audio_ends` (parametrized variants),
  `test_audio_stream_preserves_content_id`
- `test_message_history_conversion` / `test_message_history_empty_and_edge_cases` — the 50KB per-message
  and 200KB total truncation paths are **untested** in TS.
- `test_tool_result_empty_content`, `test_tool_result_after_restart_*` (idle-wait variants:
  `waits_for_response_to_finish`, `one_per_idle_turn`, `ignores_previous_reader`,
  `waits_on_next_connection_across_restart`, `sent_after_idle_timeout`, `send_failure_logged`)
- `test_receive_tracks_idle_on_current_connection`
- `test_format_tool_result_after_restart` (parametrized content shapes)

## Test harness

`src/bidi/models/__tests__/bedrock.test.ts` has a `FakeClient` that takes one response-event array
per connection (`new FakeClient([[...conn1], [...conn2]])`), records the request-body wire events in
`client.sent`, and the `chunk()` helper wraps a Nova wire event. `await tick()` after a send lets the
background request-stream drain catch up. The idle gate used by the "tool result after restart" path
is released by running `collect(model.receive())` on the new connection.

# NOTES — bidi OpenAI provider (chunk O1)

Implementation (`openai.ts`) is complete and ported line-for-line from
`strands-py/src/strands/bidi/models/openai.py`. Gates: tsc clean, eslint clean, prettier clean,
60/60 vitest tests pass.

Not yet ported from `strands-py/tests/strands/bidi/models/test_openai.py` (behaviour exists in the
implementation; only the tests are missing):

- `test_receive_preserves_native_order_with_late_transcription`
- `test_receive_combines_assistant_content` (all parametrizations)
- `test_convert_openai_event_text_and_transcript`, `test_convert_openai_event_keeps_text_separate_from_transcripts`
- `test_response_stop_with_tools`, `test_start_resets_connection_state`
- `test_response_usage_token_details` (parametrized matrix; one case covered)
- `test_update_config_warns_invalid_keys` — intentionally skipped: TS types replace Python's
  runtime key validation (`_validate_model_config` / `_validate_audio_config`).
