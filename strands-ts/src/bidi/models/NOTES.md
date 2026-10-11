# NOTES — bidi OpenAI provider (chunk O1)

Implementation (`openai.ts`) is complete and ported line-for-line from
`strands-py/src/strands/bidi/models/openai.py`. Gates: tsc clean, eslint clean, prettier clean,
41/41 vitest tests pass.

Not yet ported from `strands-py/tests/strands/bidi/models/test_openai.py` (behaviour exists in the
implementation; only the tests are missing):

- `test_receive_preserves_native_order_with_late_transcription`
- `test_receive_combines_assistant_content` (all parametrizations)
- `test_convert_openai_event_text_and_transcript`, `test_convert_openai_event_keeps_text_separate_from_transcripts`
- `test_audio_boundaries`, `test_audio_ids_distinguish_overlapping_and_reopened_streams`
- `test_response_stop_with_tools`, `test_tool_results_wait_for_response_and_pending_tools`
- `test_native_acknowledgments_correlate_inputs_and_late_transcripts`,
  `test_history_acknowledgment_is_not_new_input`, `test_receive_defers_response_during_speech`
- `test_receive_binds_websocket_per_reader`, `test_start_resets_connection_state`,
  `test_restart_waits_for_pending_tool_results` (agent-loop level in Python)
- `test_response_usage_token_details` (parametrized matrix; one case covered)
- `test_update_config_warns_invalid_keys` — intentionally skipped: TS types replace Python's
  runtime key validation (`_validate_model_config` / `_validate_audio_config`).
