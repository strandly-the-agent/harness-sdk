# `interventions::vended::steering` (strands-rs)

Branch `agent-tasks/strands-rs-steering`. Spec: `strands-ts/src/vended-interventions/steering/**`
(Python `strands-py/src/strands/vended_plugins/steering/` ties breaks). Files added:

- `crates/strands-agents/src/interventions/vended/steering/mod.rs` (module doc + divergences)
- `.../steering/context.rs` — `SteeringContextData`, `SteeringContextProvider` (+1 test)
- `.../steering/handler.rs` — `SteeringProviders`, `SteeringAction` (+5 tests)
- `.../steering/llm.rs` — `LlmSteeringHandler`, `PromptBuilder` (+11 tests)
- `.../steering/tool_ledger.rs` — `ToolLedgerProvider` (+4 tests)
- edits: `interventions/mod.rs` (`InterventionHandler::observe_agent` + registry call),
  `interventions/vended/mod.rs` (exports)

Gates (last line each):

```
cargo fmt --all                                                    -> (no output)
cargo clippy -p strands-agents --all-features --tests -- -D warnings -> Finished `dev` profile in 1m 34s
cargo test -p strands-agents --all-features --lib steering::       -> test result: ok. 21 passed; 0 failed
```

## API matrix (TS → Rust)

| TS | Rust |
| --- | --- |
| `SteeringContextData` (`{type, ...}`) | `SteeringContextData { type_name, fields }`, `new()` / `with()` |
| `SteeringContextProvider` | `trait SteeringContextProvider { name, observe_agent, context }` |
| `abstract class SteeringHandler` | `SteeringProviders` (provider plumbing) + `SteeringAction` |
| `SteeringHandlerConfig.contextProviders` | `SteeringProviders::new(providers)` |
| `getSteeringContext()` | `SteeringProviders::steering_context()` |
| `observeAgent(agent)` (`LifecycleObserver`) | `InterventionHandler::observe_agent(&Agent)` |
| `LLMSteeringHandler` | `LlmSteeringHandler` |
| config `name` / `model` / `systemPrompt` / `promptBuilder` / `contextProviders` | `LlmSteeringHandler::new(system_prompt)` + `.name()` / `.model()` / `.prompt_builder()` / `.context_providers()` |
| `'strands:llm-steering-handler'` | `DEFAULT_LLM_STEERING_NAME` |
| `PromptBuilder` | `type PromptBuilder = Arc<dyn Fn(&[SteeringContextData], Option<&ToolUse>) -> Vec<ContentBlock>>` |
| `ToolLedgerProvider` + `maxEntries` / `name` | `ToolLedgerProvider`, `DEFAULT_MAX_ENTRIES`, `DEFAULT_TOOL_LEDGER_NAME` |
| `proceed` / `guide` / `confirm` decision | `SteeringAction` → `ToolCallAction` (`Proceed` / `Cancel` with guidance / `Confirm`) |

## Test matrix (TS `it(...)` → Rust test)

### `__tests__/handler.test.ts`
| TS test | Rust test | Status |
| --- | --- | --- |
| routes beforeToolCall to subclass override with the event | `routes_before_tool_call_to_the_handler_with_the_event` | pass |
| routes afterModelCall to subclass override with the event | — | n/a (no after-model gate in Rust) |
| exposes provider context to subclasses via getSteeringContext | `exposes_provider_context_to_the_handler_via_steering_context` | pass |
| siblings with distinct names can coexist on one agent | `siblings_with_distinct_names_can_coexist_on_one_agent` | pass |
| does not invoke afterModelCall body when stopData is missing | — | n/a (no after-model gate in Rust) |
| confirm decision flows through the interrupt system on resume (approved) | `confirm_decision_flows_through_the_interrupt_system_on_resume_approved` | pass |
| confirm decision sets cancel when human denies | `confirm_decision_sets_cancel_when_human_denies` | pass |

### `__tests__/llm.test.ts`
| TS test | Rust test | Status |
| --- | --- | --- |
| defaults to the parent agent's model when none is configured | `defaults_to_the_parent_agents_model_when_none_is_configured` | pass |
| uses agent.auxModel when none is configured | `uses_the_agents_aux_model_when_none_is_configured` | pass |
| uses the configured model in preference to the agent model | `uses_the_configured_model_in_preference_to_the_agent_model` | pass |
| resolves agent.auxModel at tool-call time, so reassignment takes effect | `resolves_the_agents_model_at_tool_call_time_so_reassignment_takes_effect` | pass |

Rust-only additions in `llm.rs`: `attaches_a_tool_ledger_provider_by_default`,
`a_custom_prompt_builder_replaces_the_default`, `guide_decision_cancels_the_tool_call_with_the_models_reason`,
`confirm_decision_asks_the_human_with_the_models_reason`,
`default_prompt_says_no_context_available_and_omits_ledger_help_without_providers`,
`default_prompt_describes_a_general_evaluation_without_a_tool_call`,
`the_ledger_entry_for_the_evaluated_call_is_pending_in_the_prompt` (prompt text is verbatim from TS,
so it is asserted rather than assumed).

### `__tests__/tool-ledger.test.ts`
| TS test | Rust test | Status |
| --- | --- | --- |
| records pending entry on beforeToolCall | `records_pending_entry_on_before_tool_call` | pass |
| flips pending to success after afterToolCall | `flips_pending_to_success_after_after_tool_call` | pass |
| records error status and message | `records_error_status_and_message` | pass |
| drops oldest entries when ledger exceeds maxEntries` | `drops_oldest_entries_when_ledger_exceeds_max_entries` | pass |

## Divergences

- **Intervention, not plugin** — follows TS; Python models steering as a plugin.
- **No inheritance** — TS's abstract `SteeringHandler` splits into `SteeringProviders` (provider
  plumbing, composed by handlers) and `SteeringAction` (the narrowed proceed/guide/confirm
  contract, converted into `ToolCallAction`). A blanket
  `impl<T: SteeringHandler> InterventionHandler for T` would overlap the existing handler impls.
- **`observe_agent`** — new defaulted method on `InterventionHandler`, the port's equivalent of
  TS's `LifecycleObserver` probe; `InterventionRegistry::init_agent` calls it once per handler.
- **No `afterModelCall` steering** — Rust's `InterventionHandler` has no after-model gate.
- **Prompt builder returns `Vec<ContentBlock>`** — TS's `string | ContentBlock[]` union has no
  ergonomic Rust equivalent; `ContentBlock::text` covers the string case.

## Not yet done (next session)

- `crates/strands-agents/tests/steering_loop.rs` — agent-loop coverage with `MockMessageModel`.
- `crates/strands-agents/tests/integ_steering.rs` — live Bedrock haiku as judge + agent,
  `#[ignore = "live …"]`.
