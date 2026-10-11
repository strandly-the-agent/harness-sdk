# NOTES — chunk G1 (`GoogleGeminiLiveModel`)

Implementation is complete and ported in full (`src/bidi/models/google.ts`, exported from
`src/bidi/models/index.ts`). Gates: tsc clean, eslint clean, prettier applied, 45/45 unit tests pass.

## Test coverage gap (next agent)

`strands-py/tests/strands/bidi/models/test_google.py` is 2056 lines / ~90 tests; 45 TS tests cover
the core behaviour (config/audio resolution, connect config, history replay, every event-conversion
path, usage mapping, go-away, throttling, send paths, tool-result shapes, stop, restart/resume).
Still to port:

- Fine-grained transcript ordering: `test_completion_after_barge_in_preserves_new_utterance`,
  `test_activity_without_transcript_text_completes`,
  `test_finished_transcription_is_emitted_before_turn_complete`,
  `test_first_transcript_after_assistant_output_keeps_receive_order`.
- Agent-level restart paths: `test_proactive_restart_end_to_end_through_agent` and the
  GoAway → connection-warning/restart tests (need the bidi agent chunk to land first).
- `test_send_tool_results_preserves_group`, `test_live_tool_uses_structured_parameter_schema`,
  image-block sending, `test_start_passes_merged_config_to_genai` variants.
- `test_update_config_warns_invalid_keys` has no TS analog: there is no `validate_config_keys`
  equivalent in `strands-ts`, so unknown keys are a type error instead of a runtime warning.
