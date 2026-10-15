# Swarm matrix

`strands-ts/src/multiagent/swarm.ts` → `strands-agents::multiagent::swarm`.

## API

| TS | Rust |
| --- | --- |
| `new Swarm({ nodes, start, … })` | `Swarm::builder()…build() -> Result<Swarm>` |
| `SwarmOptions.id` / `.nodes` / `.start` | `.id(..)` / `.node(impl Into<NodeDefinition>)` / `.entry_point(id)` |
| `SwarmOptions.plugins` / `sessionManager` | `.plugin(Arc<dyn MultiAgentPlugin>)` |
| `swarm.addHook(Event, cb)` | `.hook::<E, _>(cb)` on the builder; `Swarm::hooks()` after build |
| `SwarmConfig.maxSteps` | `.max_steps(u64)`, `SwarmConfig::max_steps: Option<u64>` |
| `SwarmConfig.timeout` / `.nodeTimeout` | `.timeout(Duration)` / `.node_timeout(Duration)` |
| `SwarmConfig.repetitiveHandoffDetectionWindow` | `.repetitive_handoff_detection_window(usize)` |
| `SwarmConfig.repetitiveHandoffMinUniqueAgents` | `.repetitive_handoff_min_unique_agents(usize)` |
| `swarm.id` / `.nodes` / `.start` / `.config` | `Swarm::id()` / `nodes()` / `start()` / `config()` |
| `swarm.invoke(input, options?)` | `Swarm::invoke` / `invoke_with` (inherent), `MultiAgent::invoke_with` |
| `swarm.stream(input, options?)` | `Swarm::stream` / `stream_with` (inherent), `MultiAgent::stream_with` |
| `MultiAgentInvokeOptions.invocationState` | `MultiAgentInvokeOptions::with_invocation_state` |
| `MultiAgentInvokeOptions.cancelSignal` | `MultiAgentInvokeOptions::with_cancel(CancellationToken)` |
| `swarm.invoke([interruptResponse])` | `Swarm::invoke(vec![InterruptResponseContent::new(..)])` |
| `swarm.initialize()` | `Swarm::initialize()` |
| `HandoffResult` (interface) | `HandoffResult` (serde `camelCase`) |
| `SwarmNodeDefinition` | `NodeDefinition` (shared with Graph) |
| nested swarm as a node | `impl From<Swarm> for NodeDefinition` |
| snapshot via `SessionManager` | `Swarm::take_snapshot` / `load_snapshot` |

## Tests

Landed in increment 1 (`src/multiagent/swarm.rs` unit tests):

| TS `it` (`__tests__/swarm.test.ts`) | Rust test |
| --- | --- |
| defaults id to `'swarm'`, starts at the first node | `defaults_the_id_and_start_node` |
| throws on an empty nodes list | `rejects_an_empty_node_list` |
| throws on a duplicate agent id | `rejects_duplicate_agent_ids` |
| throws when `start` is unknown | `rejects_an_unknown_entry_point` |
| uses an explicit `start` | `resolves_an_explicit_entry_point` |
| throws when `maxSteps`/`timeout`/`nodeTimeout` < 1 | `rejects_limits_below_one` |
| repetitive-handoff pair must both be 0 or positive | `rejects_a_half_configured_repetitive_handoff_pair` |
| min unique agents cannot exceed the window | `rejects_a_uniqueness_threshold_above_the_window` |
| min unique agents cannot exceed the node count | `rejects_a_uniqueness_threshold_above_the_node_count` |
| handoff schema lists the other agents + descriptions | `builds_a_handoff_schema_listing_the_other_nodes_with_descriptions` |
| single-node swarm gets a no-targets schema | `builds_a_handoff_schema_with_no_targets_for_a_single_node_swarm` |
| detects a repetitive handoff loop | `detects_a_repetitive_handoff_loop_inside_the_window` |
| no detection when the window has enough unique nodes | `allows_enough_unique_nodes_in_the_window` |
| throws when the step limit is hit with a pending handoff | `errors_when_the_step_limit_is_reached_with_a_pending_handoff` |
| — (serde parity, Rust-only) | `serializes_a_handoff_result_like_ts` |

Landed in increment 2 (the execution loop):

| TS `it` | Rust test |
| --- | --- |
| returns completed result with content and duration | `returns_a_completed_result_with_content_and_duration` |
| hands off from A to B and returns final output | `hands_off_to_the_target_agent` |
| chains handoffs across multiple agents (A → B → C) | `chains_handoffs_across_agents` |
| passes serialized context in handoff input | `passes_the_handoff_context_as_a_pretty_json_text_block` |
| excludes current agent from handoff schema | `hands_each_agent_a_schema_that_excludes_itself` |
| throws when `maxSteps` is exceeded | `errors_when_the_step_limit_is_exceeded` |
| completes normally using exactly `maxSteps` | `completes_when_it_uses_exactly_the_step_budget` |
| failed result when handoffs miss the uniqueness threshold | `fails_the_aggregate_when_recent_handoffs_repeat` |
| checks the trailing window after diverse handoffs | `checks_the_trailing_window_after_earlier_diverse_handoffs` |
| continues when the window is diverse enough | `continues_when_the_window_has_enough_unique_nodes` |
| throws when a node exceeds `nodeTimeout` | `errors_when_a_node_exceeds_the_node_timeout` |
| applies the per-node timeout over `nodeTimeout` | `prefers_a_nodes_own_timeout_over_the_swarm_default` |
| no throw when `nodeTimeout` is `Infinity` | `runs_without_a_node_timeout` |
| throws when `timeout` is exceeded between steps | `errors_when_the_swarm_budget_runs_out_between_steps` |
| aborts an in-flight node when `timeout` expires mid-step | `aborts_an_in_flight_node_when_the_swarm_budget_runs_out` |
| — (external `cancelSignal`, no TS `it`) | `stops_at_the_loop_top_when_cancelled_externally` |
| failed result when the agent throws | `fails_the_result_when_an_agent_errors` |
| calls `initialize` only once across invocations | `initializes_once_across_invocations` |
| preserves agent messages and state after execution | `leaves_the_agent_observably_unchanged` |
| plugin initialization failure stays fail-closed | `keeps_a_plugin_initialization_failure_fail_closed` |
| yields lifecycle events in the right order | `streams_the_lifecycle_events_in_order` |
| yields a handoff event between agents | `streams_a_handoff_event_between_agents` |
| cancelled result with the default hook message | `cancels_a_node_from_a_hook_with_the_default_message` |
| cancelled result with a custom hook message | `cancels_a_node_from_a_hook_with_a_custom_message` |
| resumes from the pending handoff target after a crash | `resumes_from_the_pending_handoff_target_after_a_crash` |
| starts fresh when the previous run completed normally | `restores_only_once_so_a_completed_run_starts_fresh` |
| does not apply restored completed history to a new run | `scopes_repetitive_handoff_detection_to_the_current_invocation` |
| carries the steps count forward | `carries_forward_the_step_count_from_the_previous_invocation` |
| passes the last handoff context to the resumed node | `passes_the_last_handoff_context_to_the_resumed_node` |
| starts fresh when the resume target was removed | `starts_fresh_when_the_resume_target_left_the_swarm` |

| TS `it` (`__tests__/swarm.invocation-state.test.ts`) | Rust test |
| --- | --- |
| forwards `invocationState` to every node, mutations visible | `forwards_one_invocation_state_to_every_node` |
| defaults `invocationState` to `{}` | `defaults_the_invocation_state_to_an_empty_map` |

| TS `it` (`__tests__/interrupts.test.ts`, swarm half) | Rust test |
| --- | --- |
| Swarm: agent interrupts, resumes via `SessionManager` | `interrupts_inside_an_agent_and_resumes_from_a_restored_session` |
| Swarm: repetitive detection keeps a restored interrupt response | `repetitive_handoff_detection_does_not_swallow_a_restored_interrupt_response` |
| Swarm: `BeforeNodeCallEvent.interrupt` gates a node, then resumes | `gates_a_node_with_a_before_node_call_interrupt_and_resumes` |
| Swarm: hook-gated node still emits result + after events | `a_hook_gated_node_still_emits_its_result_and_after_events` |
| — (same-instance resume, `pending_interrupt_state`) | `resumes_an_interrupted_run_on_the_same_instance_without_a_session` |

Not ported: `swarm.tracer.test.ts` — `telemetry::Tracer` has no multi-agent or node spans yet, so
there is nothing to assert against (see **Remaining** below). TS's "per-node timeout of `Infinity`
disables a finite `nodeTimeout`" has no Rust equivalent: `AgentNodeOptions::with_timeout` takes a
`Duration`, so a node cannot express "no timeout" while the swarm sets one — `None` means "inherit".

## Remaining

- **Tracer spans.** `startMultiAgentSpan` / `startNodeSpan` / `withSpanContext` and
  `swarm.tracer.test.ts`. The tracer additions are shared with Graph, so they belong in one
  telemetry change rather than a swarm-local one.
- **`SessionManager` as a `MultiAgentPlugin`.** Not ported yet, so the resume tests install
  equivalent save/restore hooks (`Session` in the test module) over `take_snapshot` /
  `load_snapshot`, mirroring `SessionManager`'s restore-once-per-orchestrator behaviour.
- **`sessionManager` constructor argument** (and its "provided twice" validation): waiting on the
  plugin above; today a session plugin is passed through `.plugin(..)`.

## Divergences

- **Builder instead of an options struct**, matching the rest of the crate. Validation is the
  same and the messages are TS-identical, but it surfaces as `Result` from `build()` rather
  than a constructor throw.
- **`Option<T>` instead of `Infinity`** for `max_steps` / `timeout` / `node_timeout`; "below 1"
  validation becomes "is zero" (`Duration`/`u64` cannot be negative).
- **`.node(def)` takes the id from the definition** (TS `SwarmNodeDefinition` behaviour) rather
  than the `.node(id, def)` shape in the increment brief — `AgentNode`'s id already comes from
  the agent, and an id parameter that must match it buys nothing.
- **No `handoff_to_agent` tool.** TS routes with structured output and keeps routing in the
  orchestrator; the Rust port follows TS, not Python's injected tool.
- **The handoff schema is hand-written JSON Schema** (`NodeInputOptions::structured_output_schema`
  is a `serde_json::Value`), not a Zod object. `agentId` for a single-node swarm uses
  `{"not": {}}` for TS's `z.never()`.
- **`warnOnce` is a plain `tracing::warn!`** for the unbounded-execution warning; the crate has
  no warn-once helper.
- **The loop runs in a spawned task** writing to an `mpsc` channel, as `Agent::stream_with` does,
  instead of TS's async generator. Dropping the stream ends the invocation (the send fails with
  `Error::Cancelled`), and `invoke_with` is `stream_with` drained to the terminal result event.
- **Timeouts are deadlines, not `AbortSignal`s.** The swarm budget and the per-node budget are
  `tokio::time::Instant` deadlines raced against the node future in one `select!`; whichever
  expires cancels the node's `CancellationToken` (TS composes `AbortSignal.any`). Error messages
  keep TS's shape with the duration in milliseconds: `node_timeout=<20>`, `timeout=<20>`.
- **Node results are appended by the orchestrator**: `run_node` records the result on the node,
  the swarm pushes it onto `state.results` (TS `_streamNode` does both). A node that blows its own
  timeout is therefore recorded on the node state but left out of the aggregate, exactly as in TS.
- **A handoff to an unknown node id is an error** (`handoff_target=<..>, swarm_id=<..> | handoff
  target is not a node in the swarm`). TS's Zod enum rejects it inside the agent; the Rust schema
  is advisory JSON Schema, so the orchestrator checks instead of a non-null assertion.
- **A structured output that is not a `HandoffResult` ends the run** instead of being cast
  (`serde_json::from_value(..).ok()`): without Zod validation the message field can be missing,
  and ending with the node's own content is the safe reading of "no handoff".
- **An empty `state.results` resolves to empty content.** TS `_resolveContent` asserts on the last
  result and would throw a `TypeError`; this only happens when a restored state has already spent
  its step budget.
- **Resume with no leftover responses replays `MultiAgentInput::None`**, where TS replays `''`
  (`state._pendingInput ?? ''`); both mean "no new input" to the child agent.
- **`pending_interrupt_state` holds the `MultiAgentStateHandle`**, not a cloned state, so the
  retained state is the same object a TS instance would retain.
