# model-router — port matrix

Rust: `src/models/routing/{mod,router,strategy,fallback,classifier,selection}.rs` (38 unit tests)
+ `tests/model_router_loop.rs` (9 loop tests) + `tests/integ_model_router.rs` (2 live Bedrock).
TS spec: `strands-ts/src/models/routing/`; Python (`strands-py/src/strands/models/routing/`) ties
breaks on log lines.

## API

| TS | Rust |
| --- | --- |
| `new ModelRouter(models, { strategy?, maxSwitches? })` | `ModelRouter::new(Vec<RoutingCandidate>) -> Result<Self>` + `.with_strategy(impl RoutingStrategy)` / `.with_max_switches(usize)` |
| `new RoutingCandidate({ model, name?, description?, metadata? })` | `RoutingCandidate::new(Arc<dyn Model>)` / `RoutingCandidate::router(Arc<ModelRouter>)` + `.with_name` / `.with_description` / `.with_metadata(impl Serialize) -> Result<Self>` |
| `RoutingStrategy.select(context)` | `#[async_trait] RoutingStrategy::select(&self, &RoutingContext) -> Result<Option<RoutingCandidate>>` + `name()` |
| `new FallbackStrategy()` | `FallbackStrategy` (unit struct) |
| `new ClassifierStrategy(model, options)` | `ClassifierStrategy::new(Arc<dyn Model>)` + `with_system_prompt` / fallible `with_timeout` / `with_max_message_chars` / `with_max_agent_instructions_chars` / `with_max_candidate_chars` / `with_sleeper` |
| `router.getRoutedModel(agent, invocationState)` | `ModelRouter::routed_model(&Agent, &InvocationState) -> Option<Arc<dyn Model>>` |
| `new Agent({ model: router })` | `AgentBuilder::model_router(Arc<ModelRouter>)` |
| `_selectionMiddleware` on `InvokeModelStage.Input` | wrap-phase middleware on `InvokeModelStage` (see divergences) |

Re-exported from `strands_agents`: `ModelRouter`, `RoutingCandidate`, `RoutingStrategy`,
`FallbackStrategy`, `ClassifierStrategy`.

## Tests

### `router.test.ts` (20 `it`)

| TS test | Rust test | Status |
| --- | --- | --- |
| freezes candidate topology while preserving model and candidate identity | `router::tests::preserves_candidate_and_model_identity` | pass (no freeze; `Arc` identity) |
| preserves caller metadata and validates that it is a JSON-serializable object | `router::tests::preserves_caller_metadata_and_validates_that_it_is_a_json_object` | pass |
| allows subclasses to initialize strategy-specific metadata | — | n/a (no subclassing; metadata is set through `with_metadata`) |
| normalizes models and exposes stable plugin configuration | `router::tests::normalizes_models_and_exposes_stable_plugin_configuration` | pass |
| accepts a non-async select method that returns a Promise | — | n/a (`async_trait` only) |
| rejects stateful models and duplicate topology | `router::tests::rejects_stateful_models`, `rejects_duplicate_topology` (+ `rejects_empty_models`) | pass |
| selects once per agent and invocation while supplying fresh isolated request copies | `tests/model_router_loop.rs::exposes_the_routed_model_during_the_invocation` | partial (one selection per invocation is observed; the per-call request-copy isolation is not asserted) |
| keys cached state by router and agent object identity rather than caller id | — | not ported (identity keying is exercised indirectly by every loop test) |
| registers idempotent middleware, routing hook, and scoped boundary cleanup | — | not ported (boundary cleanup is observed by `tests/integ_model_router.rs`, which must read the selection from inside the loop) |
| rejects plugin initialization even when another router shares the default model | `selection::tests::rejects_a_router_registered_as_an_ordinary_plugin` | pass |
| ignores cancellation without recording a failure or selecting another candidate | `tests/model_router_loop.rs::a_cancelled_model_call_neither_records_a_failure_nor_switches` | pass (loop level: `RoutingState` only exists once the before-model hook has run, so the branch is driven through a real invocation; a cancelled model call ends the loop with `StopReason::Cancelled`, not an error) |
| records complete outcomes, switches once, and opens a new round after success | `tests/model_router_loop.rs::switches_to_the_next_candidate_when_the_model_call_fails` + `fallback::tests::rearms_after_success_while_retaining_other_candidates_failure_history` | pass (split across loop and strategy level) |
| consumes an unresolvable nested candidate and continues to a healthy candidate | `tests/model_router_loop.rs::resolves_a_nested_router_as_one_candidate` | partial (nested resolution after a failure; no unresolvable nested router) |
| stops when a strategy repeats an unresolvable candidate in the same round | — | not ported |
| does not replace a pending model error when strategy selection fails or repeats a used candidate | `tests/model_router_loop.rs::surfaces_the_error_once_every_candidate_has_failed` | partial (exhausted candidates; not a failing/repeating strategy) |
| honors maxSwitches as a cap on successful switches | `tests/model_router_loop.rs::stops_switching_at_max_switches` | pass |
| allows exactly maxSwitches successful candidate changes | `tests/model_router_loop.rs::allows_exactly_max_switches_successful_changes` | pass |
| bounds a failure round when a strategy cycles to an already-used candidate | `fallback::tests::advances_through_each_candidate_once_in_a_failure_round` | partial (strategy level) |
| keeps the successful candidate used when opening the next failure round | `fallback::tests::clears_a_successful_candidates_own_accumulated_failure_count`, `rearms_the_other_candidate_after_a_fallback_succeeds_and_later_fails` | partial (strategy level) |
| uses the default on decline and propagates invalid opening results | — | not ported |
| rejects null because only undefined declines selection | — | n/a (`Option<RoutingCandidate>`; no `null`) |
| (no TS equivalent) | `router::tests::candidate_label_falls_back_to_the_model_id`, `fallback::tests::uses_declaration_order_before_any_attempt` | extra |

Unit-test counts: `router.rs` 7, `fallback.rs` 5, `selection.rs` 1, `classifier.rs` 25 = 38.

### `classifier-strategy.test.ts` (27 `it`)

24 of 27 ported as unit tests in `classifier.rs` (full list in the session-3 status notes):
single-candidate bypass, evidence construction and budgets, synthetic-text fallbacks, instruction
joining/truncation, custom-policy framing, and every decline path (out-of-range / negative /
non-integer index, wrong tool name, invalid structured input, plain text, provider error, timeout,
abort-on-timeout), plus `rejects_a_zero_timeout` and `rejects_non_positive_character_limits`.

| TS test | Rust test | Status |
| --- | --- | --- |
| agent integration → serves candidate zero when classification fails | `tests/model_router_loop.rs::serves_candidate_zero_when_classification_fails` | pass |
| agent integration → surfaces a selected-model failure without switching | `tests/model_router_loop.rs::surfaces_a_selected_model_failure_without_switching` | pass |
| agent integration → selects an opaque nested router | `tests/model_router_loop.rs::selects_an_opaque_nested_router` | pass |
| (no TS equivalent) | `tests/model_router_loop.rs::classifier_selects_the_second_candidate` | extra |
| rejects a non-Model classifier | — | n/a (type system) |
| does not leak classifier config | — | skipped (Rust `Model` has no `getConfig`) |
| rejects non-integer / NaN / infinite option values | — | n/a (`usize` / `Duration` unrepresentable) |

Not asserted anywhere: the `tracing::warn!` text of the decline lines
(`strategy=…, reason=…, error_type=…`). `src/testing.rs` has no subscriber-capture fixture, so
those lines are covered only through behaviour (the strategy declines). Follow-up for whichever
lane adds a tracing fixture.

### Live (`tests/integ_model_router.rs`, `#[ignore = "live Bedrock"]`)

`falls_back_from_an_unknown_model_id_to_haiku` (candidate 0 = `us.anthropic.does-not-exist-v1:0`,
candidate 1 = haiku) and `classifier_picks_a_candidate_from_its_description` (haiku classifier over
two described haiku candidates). Both read the selection through `InvokeModelStage` middleware
because it is cleared at the invocation boundary. `AWS_REGION=us-west-2 cargo test
--all-features --test integ_model_router -- --ignored` → 2 passed; routed ids observed:
`["us.anthropic.does-not-exist-v1:0", "us.anthropic.claude-haiku-4-5-20251001-v1:0"]`.

## Divergences

Full list in the `routing/mod.rs` `//!` block. The load-bearing ones:

- **Selection is wrap-phase middleware**, not TS's `InvokeModelStage.Input`: `add_input` is sync
  while `RoutingStrategy::select` is async. Wrap composes innermost, so it still observes the
  context after continuation input middleware, as TS requires.
- Candidates are `Arc<dyn Model>` / `Arc<ModelRouter>`; clones share one identity, and
  `PartialEq`/`Hash` compare it (TS uses object identity + `Object.freeze`).
- Validation TS performs at runtime is type-enforced here and has no error path: candidate/strategy
  types, `maxSwitches` (`usize`), non-finite metadata numbers, and
  `auxModel must be a Model or a Bedrock model id, not a ModelRouter` (`aux_model` takes `impl Model`).
- `RoutingStrategy::name()` replaces `strategy.constructor.name` in routing logs; candidate labels
  use `Model::model_id` (no runtime provider class name).
- `ClassifierStrategy` takes fallible `with_*` builders instead of an options bag, names the Rust
  setter in its config errors (`timeout must be greater than zero`,
  `max_message_chars must be greater than zero`) where TS names the JS parameter
  (`timeoutMs must be finite and greater than zero`) — verbatim TS is kept for model-facing and
  selection-failure strings — replaces zod with an explicit `serde_json` schema, counts `char`s rather than
  UTF-16 code units, and races an injectable `Sleeper` against the classifier call, cancelling it
  through `StreamOptions::cancel` (TS's `AbortController`).
- One router cannot serve two *concurrent* invocations of the same agent: per-invocation state is
  keyed by `Agent`/router pointer identity. TS documents the identical limitation.
- Once `max_switches` is exhausted the router stops claiming the failure, so the error falls
  through to the installed retry strategy, which may retry the *same* failed candidate
  (`tests/model_router_loop.rs::retries_the_failed_candidate_once_max_switches_is_exhausted`
  pins this). Same shape as TS; deliberate.
- A configured `AgentBuilder::model_router` wins over `AgentBuilder::model` in either call order:
  `Agent::model` is derived from the router inside `build()`.
- After a candidate switch the loop's `attempt_count` keeps incrementing, so
  `ModelRetryStrategy::on_first_model_attempt` is not re-fired on a switch. TS has the same shape;
  left as is rather than changing `event_loop.rs`.

## Review disposition (round 1, review of `833705a14`)

| # | Finding | Disposition |
| --- | --- | --- |
| 🔴 | `builder.rs` `model_router()` eagerly set `self.model`, so the override only held in one call order | fixed — `model_router()` stores only the router; `build()` derives `model` from it, documented on both methods; `agent::builder::tests::the_model_router_wins_over_model_in_either_call_order` covers both orders |
| 🟠 | switch log dropped TS/Python's `error=<…>` field | fixed — `selection.rs` logs `error = <Error::name()>` alongside `model` |
| 🟠 | no loop coverage of router ↔ `DefaultModelRetryStrategy` (mocks raised non-retryable errors) | fixed — `switches_on_a_retryable_error_instead_of_retrying_the_failed_candidate` and `retries_the_failed_candidate_once_max_switches_is_exhausted` use a throttling model; the exhaustion behaviour is now asserted and documented as a deliberate TS-shaped decision |
| 🟠 | `max_switches` boundary only covered at 0 | fixed — `allows_exactly_max_switches_successful_changes` (`with_max_switches(1)`, three candidates) |
| 🟠 | `Error::Cancelled` branch untested | fixed — `a_cancelled_model_call_neither_records_a_failure_nor_switches` (loop level, see the row above for why) |
| 🟠 | config errors used JS names (`timeoutMs`, `maxMessageChars`) | fixed — Rust-named messages (`timeout must be greater than zero`, `max_*_chars must be greater than zero`); divergence recorded above. The selection-failure message `exceeding maxCandidateChars=…` stays verbatim TS |
| 🟡 | preformatted TS-style log in `classifier.rs` | fixed — structured `strategy` / `reason` / `error_type` fields |
| 🟡 | silent `"{}"` fallback when serializing the classification context | fixed — `expect` (metadata is validated to be a JSON object at construction) |
| 🟡 | inherent `ModelRouter::name()` duplicated `Plugin::name` | fixed — removed; callers resolve through the trait |
| 🟡 | `RoutingContext` froze its shape | fixed — `#[non_exhaustive]` |
| 🟡 | `model_router()` should take `impl Into<Arc<ModelRouter>>` | fixed |
| note | single `model(impl Into<ModelSource>)` alternative | not adopted — the reviewer's own recommendation; deriving `model` in `build()` is the two-line fix and leaves `model()`'s signature (and every call site/doctest) alone |
