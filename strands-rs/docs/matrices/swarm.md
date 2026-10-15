# Swarm matrix

`strands-ts/src/multiagent/swarm.ts` → `strands-agents::multiagent::swarm`.

## API

| TS | Rust |
| --- | --- |
| `new Swarm({ nodes, start, … })` | `Swarm::builder()` / `SwarmBuilder::new()` / `Default`, then `.build() -> Result<Swarm>` |
| `id ?? 'swarm'` | `DEFAULT_SWARM_ID` (re-exported from `multiagent`) |
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
| snapshot via `SessionManager` | `Swarm::take_snapshot` / `load_snapshot` (`load_snapshot(.., None)` arms the next invocation) |

## Tests

Landed in increment 1 (`src/multiagent/swarm.rs` unit tests):

| TS `it` (`__tests__/swarm.test.ts`) | Rust test |
| --- | --- |
| defaults id to `'swarm'`, starts at the first node | `defaults_the_id_and_start_node` |
| throws on an empty nodes list | `rejects_an_empty_node_list` |
| throws on a duplicate agent id | `rejects_duplicate_agent_ids` |
| throws when `start` is unknown | `rejects_an_unknown_entry_point` |
| accepts a custom id | `accepts_a_custom_id` |
| accepts `AgentNodeOptions` with per-node config | `accepts_agent_node_options_with_per_node_config` |
| defaults the limits, repetitive detection off | `defaults_the_execution_limits_and_disables_repetitive_handoff_detection` |
| uses an explicit `start` | `resolves_an_explicit_entry_point` |
| throws when `maxSteps`/`timeout`/`nodeTimeout` < 1 | `rejects_limits_below_one` |
| repetitive-handoff pair must both be 0 or positive | `rejects_a_half_configured_repetitive_handoff_pair` |
| min unique agents cannot exceed the window | `rejects_a_uniqueness_threshold_above_the_window` |
| min unique agents cannot exceed the node count | `rejects_a_uniqueness_threshold_above_the_node_count` |
| — (schema construction unit test, Rust-only) | `builds_a_handoff_schema_listing_the_other_nodes_with_descriptions` |
| — (schema construction unit test, Rust-only) | `builds_a_handoff_schema_with_no_targets_for_a_single_node_swarm` |
| — (`check_repetitive_handoff` unit test, Rust-only) | `detects_a_repetitive_handoff_loop_inside_the_window` |
| — (`check_repetitive_handoff` unit test, Rust-only) | `allows_enough_unique_nodes_in_the_window` |
| — (empty nodes list reported before a bad config) | `reports_an_empty_node_list_before_an_invalid_config` |
| — (`check_steps` unit test, Rust-only) | `errors_when_the_step_limit_is_reached_with_a_pending_handoff` |
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
| — (a streaming node must not refresh its own deadline) | `a_streaming_node_does_not_refresh_its_own_timeout` |
| applies the per-node timeout over `nodeTimeout` | `prefers_a_nodes_own_timeout_over_the_swarm_default` |
| no throw when `nodeTimeout` is `Infinity` | `runs_without_a_node_timeout` |
| per-node timeout of `Infinity` disables a finite `nodeTimeout` | `a_node_timeout_of_duration_max_opts_out_of_the_swarm_default` |
| — (swarm `timeout` of `Duration::MAX`, no TS `it`) | `a_swarm_timeout_of_duration_max_is_unbounded` |
| throws when `timeout` is exceeded between steps | `errors_when_the_swarm_budget_runs_out_between_steps` |
| aborts an in-flight node when `timeout` expires mid-step | `aborts_an_in_flight_node_when_the_swarm_budget_runs_out` |
| — (external `cancelSignal`, no TS `it`) | `stops_at_the_loop_top_when_cancelled_externally` |
| — (external `cancelSignal` mid-node, no TS `it`) | `aborts_an_in_flight_node_when_cancelled_externally` |
| — (the timer is armed after the before-invocation hook) | `arms_the_swarm_budget_after_the_before_invocation_hook` |
| — (a panicking hook reaches the `invoke_with` caller) | `propagates_a_panicking_hook_out_of_invoke` |
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
| — (two gating `BeforeNodeCallEvent` hooks, both interrupts, one resume) | `two_gating_hooks_surface_both_interrupts_and_one_resume_answers_both` |
| Swarm: hook-gated node still emits result + after events | `a_hook_gated_node_still_emits_its_result_and_after_events` |
| — (same-instance resume, `pending_interrupt_state`) | `resumes_an_interrupted_run_on_the_same_instance_without_a_session` |
| — (`load_snapshot(.., None)` arms the next invocation) | `a_snapshot_loaded_without_a_state_handle_resumes_the_next_invocation` |

Hook and stream fidelity on the branches TS exercises implicitly (no single TS `it`; the behaviour
is in `swarm.ts:411-420` and `496-523`):

| Behaviour | Rust test |
| --- | --- |
| a failing node-event hook still reports on `AfterNodeCallEvent` | `a_failing_node_event_hook_still_reports_on_the_after_node_event` |
| an unroutable resume fails before the after-invocation hook | `an_unroutable_resume_fails_before_the_after_invocation_hook` |
| a failing after-invocation hook replaces the run's own error | `a_failing_after_invocation_hook_replaces_the_runs_own_error` |
| hooks run for every event a node forwards | `dispatches_hooks_for_every_event_a_node_forwards` |

Not applicable: the two `it.each` "must be a non-negative integer" validations
(`repetitiveHandoffDetectionWindow` / `repetitiveHandoffMinUniqueAgents`) — both are `usize` in
Rust, so a negative, fractional or `NaN` value cannot be constructed.

`swarm.tracer.test.ts` is ported in `tests/multiagent_tracer.rs`, asserting the exported spans
rather than spying on the tracer: `swarm_starts_and_ends_the_multi_agent_span_on_a_successful_invocation`,
`swarm_records_the_exact_result_usage_on_the_multi_agent_span`,
`swarm_ends_a_node_span_with_completed_status_and_parents_it_to_the_multi_agent_span` (covers the
handoff-chain node spans and the `withSpanContext` parenting test),
`swarm_ends_a_node_span_with_cancelled_status_and_zero_duration_when_a_hook_cancels` and
`swarm_merges_builder_trace_attributes_onto_its_spans` (`SwarmBuilder::trace_attributes`, TS
`traceAttributes`). The null-span cases map to `telemetry::tracer::tests::ending_an_unstarted_span_is_a_no_op`
(Rust has no nullable span); the multi-hop handoff and usage-accumulation `it`s are covered by the
existing swarm execution tests plus the usage assertions above. `interrupts.test.ts:264` (a swarm
nested in a graph orchestrator) is covered in `tests/multiagent_loop.rs` — see **Cross-module and
live tests** below.

## Remaining

- **`withSpanContext`.** Not ported: the Rust tracer has no ambient span context, so a node's
  agent span is a root span instead of a child of its `node <id>` span (recorded in DESIGN.md).
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
- **An `Err` from a hook is TS's `throw`.** A hook that fails while a node's events are being
  forwarded still gets `AfterNodeCallEvent` with `error` set before the run unwinds, and a failing
  `AfterMultiAgentInvocationEvent` hook replaces the run's own error, as a throw from TS's
  `finally` does. Resume routing runs before that guarded section, so an unroutable resume fails
  without firing the after-invocation hook.
- **The loop runs in a spawned task** writing to an `mpsc` channel, as `Agent::stream_with` does,
  instead of TS's async generator. Dropping the stream ends the invocation (the send fails with
  `Error::Cancelled`), and `invoke_with` is `stream_with` drained to the terminal result event.
- **`Duration::MAX` is the Rust spelling of `Infinity`** for a timeout: a deadline that overflows
  the clock is no deadline (`deadline_after`), so a node's `Duration::MAX` opts out of the swarm's
  `node_timeout` (`AgentNodeOptions::with_timeout`, `nodes.rs`) and a swarm's leaves it unbounded.
  `None` still means "inherit the swarm's" for a node and "no limit" for the swarm.
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

## Cross-module and live tests

`tests/multiagent_loop.rs` drives a `Swarm` of real `Agent`s through the public API; the rows below
are the swarm half (the graph half is in `graph.md`).

| Behaviour | Rust test | Status |
| --- | --- | --- |
| a three-agent handoff chain: handoff events, per-node decisions, final content | `a_swarm_chains_handoffs_through_every_agent_to_a_final_answer` | ported |
| a `Graph` as a swarm node ends the swarm with its content (a nested orchestrator is handed no handoff schema, as in TS `nodes.ts`) | `a_graph_node_inside_a_swarm_ends_the_swarm_with_its_own_content` | ported |
| the pending handoff target resumes on a new instance from a serialized snapshot, carrying the in-flight handoff message | `a_swarm_resumes_its_pending_handoff_target_on_a_new_instance` | ported |
| an interrupt resumes on a new instance from a serialized snapshot | `a_swarm_interrupt_resumes_on_a_new_instance_through_a_serialized_snapshot` | `#[ignore]`d: `Agent::take_snapshot` never captures interrupt state (`src/agent/snapshot.rs:124`), so the restored node re-raises the interrupt |
| `take_snapshot(None, ..)` carries the last run's state, as DESIGN.md and `Graph` do | `a_swarm_snapshot_without_a_state_handle_carries_the_last_runs_state` | `#[ignore]`d: `Swarm::take_snapshot` passes `None` straight through (`src/multiagent/swarm.rs:176-182`), producing a stateless snapshot |
| dropping the stream cancels the node in flight | `dropping_a_swarm_stream_mid_run_cancels_the_node_in_flight` | `#[ignore]`d: `Swarm::stream_with` has no `CancelOnDrop` guard (`src/multiagent/swarm.rs:241-259`), so the run continues detached; `Graph` cancels |

`tests/integ_multiagent.rs` (live Bedrock, `#[ignore]`d):

| Behaviour | Rust test |
| --- | --- |
| a two-agent swarm where triage must hand off to the specialist | `a_two_agent_swarm_hands_off_to_the_specialist` |
| a swarm nested in a graph, handing off once inside the node | `a_graph_runs_a_swarm_node` |
