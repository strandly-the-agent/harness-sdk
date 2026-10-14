# conversation-manager — TS → Rust test matrix

Branch `agent-tasks/strands-rs-conversation-manager`, crate `strands-rs/crates/strands-agents`.
Files: `src/conversation_manager/{mod.rs,null_manager.rs,sliding_window.rs}`, wired in `src/lib.rs`.

## Counts

| TS file | TS `it(...)` | ported | skipped |
|---|---|---|---|
| `conversation-manager.test.ts` | 18 | 17 | 1 |
| `null-conversation-manager.test.ts` | 4 | 4 | 0 |
| `sliding-window-conversation-manager.test.ts` | 65 | 64 | 1 |
| **total** | **87** | **85** | **2** |

Rust module tests: **88** in `conversation_manager` (18 `mod.rs` + 4 `null_manager.rs` + 66 `sliding_window.rs`) (`cargo test --all-features` → 476 lib tests
total, 0 failed). Rust test names are the TS names in snake_case unless noted.

## conversation-manager.test.ts → `mod.rs`

| TS test | Rust test | status |
|---|---|---|
| registers both AfterModelCallEvent and BeforeModelCallEvent hooks | `registers_both_after_model_call_event_and_before_model_call_event_hooks` | pass |
| calls reduce and sets retry=true on ContextWindowOverflowError when reduce returns true | `calls_reduce_and_sets_retry_true_on_context_window_overflow_error_when_reduce_returns_true` | pass |
| does not set retry when reduce returns false | `does_not_set_retry_when_reduce_returns_false` | pass |
| does not call reduce for non-overflow errors | `does_not_call_reduce_for_non_overflow_errors` | pass |
| passes error to reduce when called due to overflow | `passes_error_to_reduce_when_called_due_to_overflow` | pass |
| always registers a BeforeModelCallEvent hook regardless of proactiveCompression setting | `always_registers_a_before_model_call_hook_regardless_of_proactive_compression_setting` | pass |
| BeforeModelCallEvent handler is a no-op when proactiveCompression is not set | `before_model_call_event_handler_is_a_no_op_when_proactive_compression_is_not_set` | pass |
| uses default threshold of 0.7 when proactiveCompression is true | `uses_default_threshold_of_0_7_when_proactive_compression_is_true` | pass |
| calls reduce without error when projected tokens exceed custom threshold | `calls_reduce_without_error_when_projected_tokens_exceed_custom_threshold` | pass |
| does not call reduce when below threshold | `does_not_call_reduce_when_below_threshold` | pass |
| does not call reduce when projectedInputTokens is undefined | `does_not_call_reduce_when_projected_input_tokens_is_undefined` | pass |
| uses 200k default when contextWindowLimit is undefined and logs warning | `uses_200k_default_when_context_window_limit_is_undefined_and_logs_warning` | pass (warning text not asserted — no tracing capture) |
| does not trigger with 200k default when below threshold | `does_not_trigger_with_200k_default_when_below_threshold` | pass |
| swallows errors from proactive reduce and continues | `swallows_errors_from_proactive_reduce_and_continues` | pass |
| throws on compressionThreshold <= 0 | `throws_on_compression_threshold_le_0` | pass |
| throws on compressionThreshold > 1 | `throws_on_compression_threshold_gt_1` | pass |
| accepts compressionThreshold of exactly 1 | `accepts_compression_threshold_of_exactly_1` | pass |
| propagates ContextWindowOverflowError out of the agent loop when reduce returns false | — | **skipped**: needs the event loop (wave 2). The hook-level half is covered by `does_not_set_retry_when_reduce_returns_false` |

Rust-only extra: `disabled_and_enabled_thresholds` (round-trips `ProactiveCompression`
→ `compression_threshold()`, the Rust enum has no direct TS analogue).

## null-conversation-manager.test.ts → `null_manager.rs`

| TS test | Rust test | status |
|---|---|---|
| does not modify conversation history on overflow | `does_not_modify_conversation_history_on_overflow` | pass |
| does not set retry on context overflow | `does_not_set_retry_on_context_overflow` | pass |
| registers both the overflow recovery and proactive compression hooks | `registers_both_the_overflow_recovery_and_proactive_compression_hooks` | pass |
| returns the plugin name | `returns_the_plugin_name` | pass |

## sliding-window-conversation-manager.test.ts → `sliding_window.rs`

### constructor / reduce / applyManagement — 10/10

| TS test | Rust test | status |
|---|---|---|
| sets default windowSize to 40 | `sets_default_window_size_to_40` | pass |
| sets default shouldTruncateResults to true | `sets_default_should_truncate_results_to_true` | pass |
| accepts custom windowSize | `accepts_custom_window_size` | pass |
| accepts custom shouldTruncateResults | `accepts_custom_should_truncate_results` | pass |
| returns true when tool results are truncated even though message count is unchanged | `returns_true_when_tool_results_are_truncated_even_though_message_count_is_unchanged` | pass |
| returns true when messages are trimmed | `returns_true_when_messages_are_trimmed` | pass |
| skips reduction when message count is less than window size | `skips_reduction_when_message_count_is_less_than_window_size` | pass |
| skips reduction when message count equals window size | `skips_reduction_when_message_count_equals_window_size` | pass |
| removes all messages when windowSize is 0 | `removes_all_messages_when_window_size_is_0` | pass |
| calls reduceContext when message count exceeds window size | `calls_reduce_context_when_message_count_exceeds_window_size` | pass |

### reduceContext — tool result truncation — 20/20

| TS test | Rust test | status |
|---|---|---|
| partially truncates large tool results preserving first and last 200 chars | `partially_truncates_large_tool_results_preserving_first_and_last_200_chars` | pass |
| preserves the durable message id when truncating tool results | `preserves_the_durable_message_id_when_truncating_tool_results` | pass |
| leaves small tool results unchanged | `leaves_small_tool_results_unchanged` | pass |
| finds oldest message with tool results | `finds_oldest_message_with_tool_results` | pass |
| returns after successful truncation without trimming messages | `returns_after_successful_truncation_without_trimming_messages` | pass |
| skips truncation when shouldTruncateResults is false | `skips_truncation_when_should_truncate_results_is_false` | pass |
| does not re-truncate already-truncated results | `does_not_re_truncate_already_truncated_results` | pass |
| replaces image blocks nested in tool results with descriptive placeholders | `replaces_image_blocks_nested_in_tool_results_with_descriptive_placeholders` | pass |
| preserves the error field on truncated tool results | `preserves_the_error_field_on_truncated_tool_results` | pass |
| image placeholder reflects non-bytes source kinds honestly | `image_placeholder_reflects_non_bytes_source_kinds_honestly` | pass |
| replaces video bytes blocks with a descriptive placeholder | `replaces_video_bytes_blocks_with_a_descriptive_placeholder` | pass |
| replaces video s3 blocks with a descriptive placeholder | `replaces_video_s3_blocks_with_a_descriptive_placeholder` | pass |
| replaces document bytes blocks with a descriptive placeholder | `replaces_document_bytes_blocks_with_a_descriptive_placeholder` | pass |
| replaces document s3 blocks with a descriptive placeholder | `replaces_document_s3_blocks_with_a_descriptive_placeholder` | pass |
| partially truncates large text inside a document text source | `partially_truncates_large_text_inside_a_document_text_source` | pass |
| leaves small text inside a document text source unchanged | `leaves_small_text_inside_a_document_text_source_unchanged` | pass |
| truncates long nested text blocks inside a document content source | `truncates_long_nested_text_blocks_inside_a_document_content_source` | pass |
| replaces large json blocks with a size placeholder | `replaces_large_json_blocks_with_a_size_placeholder` | pass |
| leaves small json blocks unchanged | `leaves_small_json_blocks_unchanged` | pass |
| does not call truncateToolResults unless an error is passed in | `does_not_call_truncate_tool_results_unless_an_error_is_passed_in` | pass |

Rust-only extra: `truncation_marker_slices_on_utf16_code_units` — proves the marker slices on
UTF-16 code units, not Rust `char`s or bytes (200 astral-plane emoji + 100 ASCII + 200 emoji →
`chars="500"`, 100 emoji kept each side). TS gets this for free from JS strings; in Rust it is a
real behavioural invariant worth pinning.

### reduceContext — message trimming — 5/5 (all added this run)

| TS test | Rust test | status |
|---|---|---|
| trims oldest messages when tool results cannot be truncated | `trims_oldest_messages_when_tool_results_cannot_be_truncated` | pass |
| calculates correct trim index (messages.length - windowSize) | `calculates_correct_trim_index_messages_length_minus_window_size` | pass |
| removes all messages when windowSize is 0 on context overflow | `removes_all_messages_when_window_size_is_0_on_context_overflow` | pass |
| uses default trim index of 2 when messages <= windowSize | `uses_default_trim_index_of_2_when_messages_le_window_size` | pass |
| removes messages from start of array using splice | `removes_messages_from_start_of_array_using_splice` | pass |

### reduceContext — tool pair validation — 18/19

| TS test | Rust test | status |
|---|---|---|
| does not trim at index where oldest message is toolResult | `does_not_trim_at_index_where_oldest_message_is_tool_result` | pass |
| does not trim at index where oldest message is toolUse without following toolResult | `does_not_trim_at_index_where_oldest_message_is_tool_use_without_following_tool_result` | pass |
| allows trim when oldest message is text or other non-tool content | `allows_trim_when_oldest_message_is_text_or_other_non_tool_content` | pass |
| skips assistant message to ensure trimmed conversation starts with user | `skips_assistant_message_to_ensure_trimmed_conversation_starts_with_user` | pass |
| returns false when no valid trim point exists | `returns_false_when_no_valid_trim_point_exists` | pass |
| allows trim when oldest message is toolUse with following toolResult | `allows_trim_when_oldest_message_is_tool_use_with_following_tool_result` | pass |
| allows trim at toolUse when toolResult immediately follows | `allows_trim_at_tool_use_when_tool_result_immediately_follows` | pass |
| falls back to a complete tool pair when no plain user trim point exists | `falls_back_to_a_complete_tool_pair_when_no_plain_user_trim_point_exists` | pass |
| keeps routine window management user-first when it uses the tool-pair fallback | `keeps_routine_window_management_user_first_when_it_uses_the_tool_pair_fallback` | pass |
| prefers a plain user trim point over a reachable complete tool pair later in range | `prefers_a_plain_user_trim_point_over_a_reachable_complete_tool_pair_later_in_range` | pass |
| does not treat a user message containing tool use as an assistant tool-pair boundary | `does_not_treat_a_user_message_containing_tool_use_as_an_assistant_tool_pair_boundary` | pass |
| returns false when an assistant tool use is not followed by a tool result | `returns_false_when_an_assistant_tool_use_is_not_followed_by_a_tool_result` | pass |
| uses the pinned first user as a safe fallback anchor | `uses_the_pinned_first_user_as_a_safe_fallback_anchor` | pass |
| declines fallback when a pinned prefix ends with an assistant message | `declines_fallback_when_a_pinned_prefix_ends_with_an_assistant_message` | pass |
| declines fallback when non-contiguous pinned messages do not alternate | `declines_fallback_when_non_contiguous_pinned_messages_do_not_alternate` | pass |
| declines fallback when the first pinned message is an orphaned tool result | `declines_fallback_when_the_first_pinned_message_is_an_orphaned_tool_result` | pass |
| declines fallback when the last pinned message carries a bare tool use block | `declines_fallback_when_the_last_pinned_message_carries_a_bare_tool_use_block` | pass |
| declines fallback when no plain user message exists before the tool pair | `declines_fallback_when_no_plain_user_message_exists_before_the_tool_pair` | pass |
| propagates the original ContextWindowOverflowError when reduce cannot reduce further | — | **skipped: needs the event loop** (hook-level half covered by `returns_false_when_no_valid_trim_point_exists`) |

The *implementation* of every one of these paths exists
(`find_tool_pair_trim_point`, `find_tool_pair_user_anchor`, `is_pinned`,
`sliding_window.rs:338-380`) and is now covered: the 13 tool-pair/fallback-anchor tests were added
in a follow-up run and all pass against the implementation unchanged — no behaviour bug was found.

### helper methods — 6/6 (all added this run)

| TS test | Rust test | status |
|---|---|---|
| findOldestMessageWithToolResults: returns correct index when tool results exist | `returns_correct_index_when_tool_results_exist` | pass |
| findOldestMessageWithToolResults: returns undefined when no tool results exist | `returns_undefined_when_no_tool_results_exist` | pass |
| findOldestMessageWithToolResults: iterates forward from start | `iterates_forward_from_start` | pass |
| truncateToolResults: returns true when changes are made | `returns_true_when_changes_are_made` | pass |
| truncateToolResults: returns false when already truncated | `returns_false_when_already_truncated` | pass |
| truncateToolResults: returns false when no tool results found | `returns_false_when_no_tool_results_found` | pass |

### reduceOnThreshold — 2/2

| TS test | Rust test | status |
|---|---|---|
| trims oldest messages when compressionThreshold is exceeded | `trims_oldest_messages_when_compression_threshold_is_exceeded` | pass |
| does not trim when below compressionThreshold | `does_not_trim_when_below_compression_threshold` | pass |

### pinFirst — 3/3 (+1 Rust-only)

| TS test | Rust test | status |
|---|---|---|
| protects first N messages from trimming | `protects_first_n_messages_from_trimming` | pass |
| returns false when all messages in trim range are protected | `returns_false_when_all_messages_in_trim_range_are_protected` | pass |
| pinned message in middle of window survives trimming | `pinned_message_in_middle_of_window_survives_trimming` | pass |

Rust-only extra: `pin_first_latch_applies_only_once` — the `AtomicBool` latch in `Inner` is shared
across clones, which TS gets from instance state; worth a dedicated test.

## Deviations from TS

1. **`test_agent_with_messages` is a module-local shim.** This branch originally rewrote
   `agent/mod.rs`'s fixtures as `test_support::test_agent_with_messages`, which has since diverged
   from the integration branch. `agent/mod.rs` is now restored byte-for-byte from
   `agent-tasks/strands-rs` (`test_agent_with(id, system_prompt, messages)` / `test_agent()` /
   `test_agent_for_session(id)`) and the one-line shim lives in
   `conversation_manager::test_support`, so the merge is clean. Only additive change to
   `agent/mod.rs`: `#[allow(dead_code)]` on `mod test_fixtures`, needed because `test_agent` and
   `test_agent_for_session` have no caller on *this* branch (session tests use them on theirs).
   **Drop that attribute at merge.**
2. **No log-assertion tests.** TS asserts on `console.warn` / logger spies (e.g. "logs warning" for
   the 200k default). Rust emits via `tracing` with no capture layer wired in, so those tests assert
   the *behaviour* and not the log line. Noted inline above.
3. **Error-propagation tests are hook-level, not loop-level.** The two TS tests that drive a real
   `agent()` invocation and assert the `ContextWindowOverflowError` escapes are replaced by
   asserting `event.retry == false` after the `AfterModelCallEvent` hook — the closest observable
   the hook layer offers.
4. **`ProactiveCompression` is an enum, not `boolean | {compressionThreshold}`.** TS's union is
   modelled as `ProactiveCompression::{Disabled, Default, Threshold(f64)}`; validation
   (`0 < t <= 1`) moved from the constructor's `throw` to `with_config() -> Result<Self>`. The three
   threshold-validation TS tests assert on the `Err` instead of a panic.
5. **UTF-16 slicing is explicit.** `utf16_len` / `encode_utf16` in `sliding_window.rs:385-400`
   reproduce JS string semantics for `PRESERVE_CHARS`, the `<truncated chars="N"/>` count and the
   `[json: N chars]` placeholder. Rust `char`/byte slicing would give different numbers for
   non-BMP text; `truncation_marker_slices_on_utf16_code_units` pins this.

## Needs from the agent loop (wave 2)

- `propagates ContextWindowOverflowError out of the agent loop when reduce returns false`
  (`conversation-manager.test.ts:388`) and `propagates the original ContextWindowOverflowError when
  reduce cannot reduce further` (`sliding-window…test.ts:1355`) — both require the loop to turn
  `retry == false` into a returned `Err`. Add as `tests/` integration tests once the loop lands.
- End-to-end proactive compression: the loop must populate
  `BeforeModelCallEvent::projected_input_tokens` from real token accounting. Current tests inject it
  by hand, so the *wiring* (does the loop actually set it?) is untested here.
- `attempt_count` / retry-latch interaction: `AfterModelCallEvent::attempt_count` is hard-coded to
  `1` in the test fixture. Whether a second overflow on the same invocation re-enters `reduce` is
  loop policy and is not observable from the hook alone.

## Open points for the reviewer

- The 17 previously-unported sliding-window tests (13 tool-pair/fallback-anchor, 2 reduceOnThreshold,
  2 pinFirst) are now written and passing. The only remaining sliding-window skip is
  `propagates the original ContextWindowOverflowError when reduce cannot reduce further`, which
  needs the wave-2 event loop.
- Two TS assertions were dropped deliberately: `declines fallback when a pinned prefix ends with an
  assistant message` spies on the `logger.debug`/`logger.warn` text, which has no `tracing`
  equivalent here, so the Rust test asserts the observable result (no reduction, history intact)
  instead. Pinning tests compare roles/texts rather than whole `Message` values because
  `pinFirst` writes a `pinned` metadata marker into the messages it protects.
- `apply_pin_first` / `is_pinned` use a message-level pin marker; confirm it matches the session
  manager's durable-id scheme (03c) before merge.
