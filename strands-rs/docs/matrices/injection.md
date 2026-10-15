# Injection module — TS → Rust port matrix

TS source: `strands-ts/src/injection/{index,message-injection,types,xml}.ts`,
tests `strands-ts/src/injection/__tests__/message-injection.test.ts`.
Rust: `strands-rs/crates/strands-agents/src/injection/{mod,message_injection,types,xml}.rs`.

## API

| TS                                   | Rust                                                        | Notes                                                                                          |
| ------------------------------------ | ----------------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| `escapeXmlText(value)`               | `injection::escape_xml_text(&str) -> String`                | Single home; `memory::injection` re-exports instead of keeping its own copy                     |
| `escapeXmlAttr(value)`               | `injection::escape_xml_attr(&str) -> String`                | ditto                                                                                           |
| `isUserTurn(messages)`               | `injection::is_user_turn(&[Message]) -> bool`               |                                                                                                 |
| `foldIntoLastUserMessage(msgs, text)`| `injection::fold_into_last_user_message(&[Message], &str) -> FoldOutcome` | TS `{ messages, appended }` → named struct `FoldOutcome { messages, appended }`    |
| `InjectionTrigger` (`'userTurn'`…)   | `injection::InjectionTrigger` enum                          | `UserTurn` / `EveryTurn`                                                                        |
| `InjectionContext`                   | `injection::InjectionContext`                               | Carries `messages`, agent handle, app state                                                     |
| `resolveTrigger(trigger)`            | `injection::Trigger` (`builtin`/`predicate`/`should_inject`) | Resolution is a value, not a function returning a closure — avoids boxing at every call site     |
| `InjectionConfig` / `createInjectionMiddleware(opts)` | `injection::MessageInjection` (+ `RenderFn`)  | Engine struct with `inject(...)`; see divergence below                                          |

## Tests

| TS `it(...)`                                                        | Rust test                                                                         |
| ------------------------------------------------------------------- | --------------------------------------------------------------------------------- |
| appends the text as a trailing TextBlock…                            | `appends_the_text_as_a_trailing_text_block_on_the_last_user_message`              |
| preserves the folded-into message tracking id                        | `preserves_the_folded_into_message_tracking_id`                                   |
| returns a new array and does not mutate the input                    | `leaves_every_other_message_untouched`                                            |
| appends after the tool result block…                                 | `appends_after_the_tool_result_block_when_the_target_is_a_tool_result_turn`        |
| targets the most recent user message when several exist              | `targets_the_most_recent_user_message_when_several_exist`                          |
| preserves message metadata on the folded message                     | `preserves_message_metadata_on_the_folded_message`                                |
| returns the input unchanged when there is no user message            | `returns_the_input_unchanged_when_there_is_no_user_message`                        |
| reports no per-call trailing blocks when the target is not last      | `reports_no_per_call_trailing_blocks_when_the_fold_target_is_not_the_last_message` |
| omits the blank-line separator when the target has no content        | `omits_the_blank_line_separator_when_the_target_message_has_no_content`            |
| `isUserTurn` (4 cases)                                               | `is_user_turn_distinguishes_a_fresh_ask` (table-driven over the 4 cases)           |
| `resolveTrigger` defaults / `userTurn` / `everyTurn` / custom        | `defaults_to_the_user_turn_policy`, `user_turn_uses_is_user_turn`, `every_turn_always_fires`, `uses_a_custom_predicate_over_the_context` |
| fails open when a custom predicate throws                            | n/a — a Rust `Trigger` predicate returns `bool` and cannot throw                   |
| folds renderContent() text into the latest user message              | `folds_rendered_text_into_the_latest_user_message`                                 |
| passes an InjectionContext carrying the conversation                 | `passes_the_conversation_and_the_agent_to_the_renderer`                            |
| exposes appState and the agent on the InjectionContext               | ditto (one test covers both fields)                                                |
| returns the context unchanged when the trigger does not fire         | `returns_the_messages_unchanged_when_the_trigger_does_not_fire`                     |
| `everyTurn` injects on an autonomous tool-result turn                | `every_turn_injects_on_a_tool_result_turn_keeping_the_tool_result_first`            |
| returns the context unchanged when renderContent yields empty text   | `blank_or_absent_text_is_a_no_op`                                                  |
| fails open when renderContent throws                                 | `fails_open_when_the_renderer_errors`                                              |
| does not mutate the original context messages                        | covered by `leaves_every_other_message_untouched` + fold tests                      |
| reports one trailing block / accumulates across producers            | `accumulates_trailing_blocks_across_producers_on_one_call`                          |
| reports nothing when skipped / when target is not last               | `returns_the_messages_unchanged_when_the_trigger_does_not_fire`, `reports_no_per_call_trailing_blocks_when_the_fold_target_is_not_the_last_message` |
| emits no cache point into the messages                               | `emits_no_cache_point_into_the_messages`                                           |
| —                                                                    | `a_custom_trigger_predicate_gates_the_injection`, `debug_names_the_policy_without_leaking_the_predicate`, xml escaping (2) |

Agent-loop coverage: `crates/strands-agents/tests/injection_loop.rs` (injection reaches the model
call that triggered it; durable history untouched; the default `UserTurn` trigger skips a
tool-result turn).

## Divergences

- **Delivery seam.** TS ships injection as an `InvokeModelStage.Input` middleware handler. Rust
  uses the equivalent hook, `BeforeModelCallEvent`: its `messages` are the per-call copy and its
  `dynamic_trailing_blocks` carries TS's `dynamicTrailingBlocks` accumulator through to cache-point
  placement. Same ordering guarantees, no second middleware mechanism.
- **`resolveTrigger` → `Trigger` value.** A resolved policy is a value with `should_inject(&ctx)`
  rather than a returned closure; `Debug` names the policy (`Trigger(UserTurn)` /
  `Trigger(predicate)`) without leaking the predicate.
- **Fail-open surface.** Only the renderer can fail in Rust (`RenderFn` returns `Result`), so the
  TS "predicate throws" case has no Rust analogue; the renderer-error path keeps TS's fail-open
  behaviour (injection skipped, call proceeds).
- **XML helpers are owned here.** `memory::injection` previously carried its own copies; it now
  re-exports these. The public `memory` API is unchanged.
