# decisions — test & API matrix (TS → Rust)

TS source: `strands-ts/src/decisions/` (`types.ts`, `decision-model.ts`, `llm-decision-model.ts`).
Rust: `crates/strands-agents/src/decisions/{mod,types,decision_model,llm_decision_model}.rs`,
`crates/strands-agents/tests/{decisions_loop,integ_decisions}.rs`.
No Python counterpart exists, so TS is the only spec.

Run with:

```text
cargo test --all-features -p strands-agents decisions           # 33 unit tests
cargo test --all-features -p strands-agents --test decisions_loop   # 10 e2e tests
cargo test --all-features -p strands-agents --test integ_decisions -- --ignored  # 1 live Bedrock
```

## `__tests__/llm-decision-model.test.ts` → `src/decisions/`

| TS test | Rust test | Status |
| --- | --- | --- |
| `narrows string and bool answers from a successful tool call` | `llm_decision_model::tests::narrows_string_and_bool_answers_from_a_successful_tool_call` | pass |
| `falls back to wall-clock latency when the provider omits latency metrics` | `llm_decision_model::tests::falls_back_to_wall_clock_latency_when_the_provider_omits_latency_metrics` | pass |
| `returns Uncertain when the model picks the uncertain option` | `llm_decision_model::tests::returns_uncertain_when_the_model_picks_the_uncertain_option` | pass |
| `returns Uncertain carrying the model-supplied reason` | `llm_decision_model::tests::returns_uncertain_carrying_the_model_supplied_reason` | pass |
| `omits uncertain from the enum when a question opts out` | `llm_decision_model::tests::omits_uncertain_from_the_enum_when_a_question_opts_out` | pass |
| `appends uncertain to the enum by default` | `llm_decision_model::tests::appends_uncertain_to_the_enum_by_default` | pass |
| `forces the structured-output tool` | `llm_decision_model::tests::forces_the_structured_output_tool` | pass |
| `wraps a plain string state as a single user message` | `llm_decision_model::tests::wraps_a_plain_string_state_as_a_single_user_message` | pass |
| `wraps a content array as a single user message` | `llm_decision_model::tests::wraps_a_content_array_as_a_single_user_message` | pass |
| `passes a message history straight through` | `llm_decision_model::tests::passes_a_message_history_straight_through` | pass |
| `includes the untrusted-state defense in the system prompt` | `llm_decision_model::tests::includes_the_untrusted_state_defense_in_the_system_prompt` | pass |
| `throws when the model returns a non-tool-use stop` | `llm_decision_model::tests::errors_when_the_model_returns_a_non_tool_use_stop` | pass |
| `rejects an empty question map before calling the model` | `decision_model::tests::ask_validates_before_delegating` (case 1) | pass |
| `rejects a missing or empty question text` | `decision_model::tests::ask_validates_before_delegating` (case 2) | pass |
| `rejects an uncertainty value that is neither boolean nor object` | — | not applicable: `Uncertainty` is an enum, so the invalid shape cannot be constructed |
| `rejects out-of-range minConfidence` | `decision_model::tests::ask_validates_before_delegating` (last case) + `an_uncertainty_at_the_range_bounds_is_valid` | pass |
| `rejects a choice literally named 'uncertain'` | `decision_model::tests::ask_validates_before_delegating` (reserved-literal case) | pass |
| `rejects duplicate option values` | `decision_model::tests::ask_validates_before_delegating` (duplicate case) | pass |
| `returns a result without usage when the provider omits it` | `llm_decision_model::tests::returns_a_result_without_usage_when_the_provider_omits_it` | pass |
| `throws when the model returns an unknown option` | `llm_decision_model::tests::errors_when_the_model_returns_an_unknown_option` | pass |
| `construction > rejects a non-Model argument` | — | not applicable: `model(impl Model + 'static)` is checked at compile time |

Rust-only unit tests (no TS counterpart; they pin behaviour TS gets from zod/`toJSON`):
`the_answer_schema_matches_the_typescript_zod_output`, `the_answer_schema_describes_every_question_in_order`,
`the_default_system_prompt_appends_the_mandatory_rules`, `the_builder_sets_the_model_and_the_system_prompt`,
`errors_when_the_model_omits_an_answer`, `validation_rejects_questions_before_calling_the_model`,
`ask_delegates_valid_questions_and_returns_the_result`, `ask_is_callable_through_a_trait_object`,
and the `types::tests::*` serde tests (`uncertain_serializes_like_ts_to_json`,
`decision_result_serializes_like_ts`, `answer_serializes_untagged_like_ts`,
`questions_preserve_insertion_order`, …).

## `__tests__/llm-decision-model.tracing.test.node.ts` → `tests/decisions_loop.rs`

Both TS tracing `it`s collapse into one Rust test, because the Rust decision span is created by
`DecisionModelExt::ask` (never by the caller) and therefore has no parent in either case — the
"no active parent span" assertion is the same assertion as "the decision span is a root".

| TS test | Rust test | Status |
| --- | --- | --- |
| `parents child spans created inside _ask under the decision span` | `tracing::the_decision_span_parents_child_spans_and_records_the_answers` (`#[cfg(feature = "telemetry")]`) | pass |
| `runs when there is no active parent span` | same test (`decision_span.parent_span_id == SpanId::INVALID`) | pass |

## `tests/decisions_loop.rs` (cross-module e2e, `MockMessageModel`)

| Rust test | Covers |
| --- | --- |
| `answers_boolean_and_choice_questions_in_one_call` | boolean + choice in one call, usage, latency, single model call |
| `forces_the_structured_output_tool_and_the_hardened_system_prompt` | forced `ToolChoice::Tool`, one tool spec, user message, mandatory rules appended |
| `returns_uncertain_with_a_reason_when_the_model_opts_out` | `Answer::Uncertain` + reason, `"uncertain"` appended to the schema enum |
| `opting_out_of_uncertainty_removes_the_option_from_the_schema` | `Uncertainty::Disabled` |
| `falls_back_to_wall_clock_latency_when_the_provider_omits_metrics` | no metadata event → wall-clock latency, `usage == None` |
| `an_invalid_question_set_fails_before_the_model_is_called` | validation error, `call_count() == 0` |
| `an_answer_outside_the_allowed_options_is_an_error` | unknown option |
| `a_text_only_turn_is_an_error` | non-tool-use stop |
| `a_decision_model_is_usable_behind_a_trait_object` | `Arc<dyn DecisionModel>` |
| `tracing::the_decision_span_parents_child_spans_and_records_the_answers` | span tree + `gen_ai.*` attributes/events |

`tests/integ_decisions.rs::answers_boolean_and_choice_questions_on_live_bedrock` —
`#[ignore = "live Bedrock"]`, `us.anthropic.claude-haiku-4-5-20251001-v1:0`; prints
`skipping: no usable AWS credentials (…)` and returns when the client cannot be built.

## API map

| TS | Rust | Note |
| --- | --- | --- |
| `abstract class DecisionModel` + `protected _ask` | `trait DecisionModel { async fn decide }` + `trait DecisionModelExt { async fn ask }` | the traced/validating wrapper is a blanket impl, so it cannot be overridden |
| `ask<const Q>(state, questions): Promise<DecisionResult<Q>>` | `ask(impl Into<DecisionInput>, &Questions) -> Result<DecisionResult>` | const-generic narrowing → `Answer` enum |
| `DecisionInput = string \| ContentBlock[] \| Message[]` | `enum DecisionInput { Text, Blocks, Messages }` + `From` impls | |
| `interface Question<C>` | `enum Question { Boolean { .. }, Choice { choices, .. } }` | `choices: 'boolean'` is a variant, not a magic string |
| `uncertainty?: boolean \| { minConfidence }` | `enum Uncertainty { Allowed, Disabled, MinConfidence(f64) }` | |
| `Record<string, Question>` | `struct Questions(IndexMap<String, Question>)` with `.boolean()/.choice()/.with()` | insertion order is the schema order, as in TS |
| `defineQuestions(...)` | — | not needed: `Questions` is already a value |
| `class Uncertain { reason? }` | `struct Uncertain { reason: Option<String> }` | `Display` matches TS `toString()` |
| `AnswerOf<Q>` | `enum Answer { Bool, Choice(String), Uncertain(Uncertain) }` + `as_bool/as_choice/as_uncertain` | serialises untagged, like TS |
| `DecisionResult<Q>` | `struct DecisionResult { answers, usage, metadata }` + `.answer(id)` | `toJSON()` parity asserted |
| `class LLMDecisionModel` / `LLMDecisionModelOptions` | `LlmDecisionModel::new(impl Model)` / `LlmDecisionModel::builder()` | `Llm` casing per crate convention; builder mirrors `AgentBuilder` |

## Divergences

1. **No const-generic narrowing.** TS returns `boolean` for `choices: 'boolean'` and the tuple
   union for a choice list. Rust has no equivalent of a `const` type parameter over a map of
   heterogeneous questions, so answers are an `Answer` enum with `as_bool`/`as_choice`/
   `as_uncertain` accessors. This is the shape `docs/PLAN_PHASE5.md` prescribes.
2. **`ask` is an extension trait.** TS makes `ask` a concrete method on the abstract class and
   `_ask` abstract. Rust's object-safe equivalent is `DecisionModel::decide` plus a blanket
   `DecisionModelExt::ask`, which keeps validation + tracing unskippable and `dyn DecisionModel`
   usable (`ask` takes `impl Into<DecisionInput>`, so it cannot live on the object-safe trait).
3. **Validation errors are `Error::Other`** with the TS message text (`question id=<id> | …`), not
   a dedicated error variant — matching how the rest of the crate reports argument validation.
4. **Uncertainty opt-out only affects the schema.** Same as TS: `Uncertainty::Disabled` removes
   `"uncertain"` from the model's enum; it does not change the answer type.
5. **The two TS tracing tests collapse into one** (see above).
