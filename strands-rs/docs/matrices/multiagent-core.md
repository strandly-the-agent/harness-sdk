# Multi-agent core — port matrix

TS spec: `strands-ts/src/multiagent/{state,multiagent,events}.ts` + `__tests__/{state,events}.test.ts`.
Rust: `crates/strands-agents/src/multiagent/{state,multiagent,events,nodes}.rs`.

## API

| TS | Rust |
| --- | --- |
| `Status` (const object + union) | `state::Status` (enum, serde SCREAMING) |
| `ResultStatus` | `state::ResultStatus` |
| `NodeResult` + `toJSON`/`fromJSON` | `state::NodeResult` (serde `type: "nodeResult"`) |
| `NodeResultUpdate` | `state::NodeResultUpdate` + `NodeResult::merge` |
| `NodeState` (+ `content` getter) | `state::NodeState` (+ `content()`) |
| `MultiAgentResult` (+ `_resolveStatus`, `_aggregateNodeUsage`) | `state::MultiAgentResult`, `resolve_status`, `aggregate_node_usage` |
| `MultiAgentState` (+ `node`, `nodes`, `_pendingInput`) | `state::MultiAgentState` (+ `node`, `node_mut`, `node_or_insert`, `nodes`, `pending_input`) |
| — (TS passes the state object directly) | `state::MultiAgentStateHandle` |
| `MultiAgentInput` / `MultiAgentContentInput` | `multiagent::MultiAgentInput` (+ `resume_responses`) |
| `MultiAgentInvokeOptions` | `multiagent::MultiAgentInvokeOptions` |
| `MultiAgent` interface | `multiagent::MultiAgent` + `MultiAgentExt` |
| `AsyncGenerator<MultiAgentStreamEvent, ..>` | `multiagent::MultiAgentStream` |
| `extractResumeResponses` | `MultiAgentInput::resume_responses` (typed enum makes the mixed-content check unnecessary) |
| `groupInterruptResponsesByNode` | `multiagent::group_interrupt_responses_by_node` |
| `dropStaleInterruptedResult` | `multiagent::drop_stale_interrupted_result` |
| `recordHookInterrupt` | `multiagent::record_hook_interrupt` |
| `applyOrchestratorHookResponses` | `multiagent::apply_orchestrator_hook_responses` |
| — | `multiagent::interrupted_result` (shared INTERRUPTED aggregate construction) |
| the ten `*Event` classes | `events::*` (same names), each `impl HookEvent` |
| `interruptFromMultiAgentNode` via `BeforeNodeCallEvent.interrupt` | `events::BeforeNodeCallEvent::interrupt` |
| `NodeStreamUpdateInnerEvent` | `events::NodeInnerEvent` |
| `MultiAgentStreamEvent` union | `events::MultiAgentStreamEvent` enum |
| `NodeType` | `nodes::NodeType` |
| `Node` abstract class (`stream` + abstract `handle`) | `nodes::Node` trait (`handle` only) + the free fn `nodes::run_node` (the `stream` wrapper) |
| `NodeConfig` / `NodeInputOptions` | `nodes::NodeConfig` / `nodes::NodeInputOptions` (no `buffer_output`) |
| `AgentNode` / `AgentNodeOptions` | `nodes::AgentNode` / `nodes::AgentNodeOptions` (`timeout: Option<Duration>`) |
| `MultiAgentNode` / `MultiAgentNodeOptions` | `nodes::MultiAgentNode` / `nodes::MultiAgentNodeOptions` |
| `NodeDefinition` union | `nodes::NodeDefinition` enum + `From` impls + `into_node` |
| `EdgeHandler` | `edge::EdgeHandler` (+ `edge::edge_handler` for sync predicates) |
| `Edge` | `edge::Edge` (+ `should_traverse`) |
| `EdgeOptions` / `EdgeDefinition` | `edge::EdgeDefinition` (one struct; `From<(S, T)>` for the tuple form) |
| `MultiAgentPlugin` | `plugins::MultiAgentPlugin` (`init_multi_agent`) |
| `MultiAgentPluginRegistry` | `plugins::MultiAgentPluginRegistry` (`new`/`add`/`get`/`initialize`) |
| `TakeMultiAgentSnapshotOptions` | `snapshot::TakeMultiAgentSnapshotOptions` |
| `takeSnapshot` / `loadSnapshot` | `snapshot::take_snapshot` / `snapshot::load_snapshot` (over `&dyn MultiAgent`) |

## Tests

`__tests__/state.test.ts` → `state.rs::tests` (NodeResult / NodeState / MultiAgentResult /
MultiAgentState round-trips, status precedence, `_pendingInput` rehydration) and
`multiagent.rs::tests` (response grouping, resume detection).
`__tests__/events.test.ts` → `events.rs::tests` (construction, `_shouldReverseCallbacks` →
`reverse_callbacks`, `toJSON` → `type_name`, `cancel`, hook interrupts).

| `__tests__/nodes.test.ts` `it` | Rust test in `nodes.rs::tests` |
| --- | --- |
| returns COMPLETED NodeResult on successful execution | `returns_completed_node_result_on_successful_execution` |
| catches errors and returns FAILED NodeResult | `catches_errors_and_returns_failed_node_result` |
| — (TS covers cancel mapping in graph tests) | `maps_an_error_to_cancelled_when_the_run_was_cancelled` |
| throws when timeout < 1 | `rejects_a_zero_timeout` |
| accepts a positive timeout | `accepts_a_positive_timeout` |
| accepts Infinity as an explicit opt-out | `accepts_duration_max_as_an_explicit_timeout_opt_out` |
| defaults preserveContext to false | `defaults_preserve_context_to_false` |
| stores the preserveContext flag when provided | `stores_the_preserve_context_flag_when_provided` |
| throws when preserveContext is set with a non-Agent InvokableAgent | not applicable — `AgentNode` wraps the concrete `Agent` |
| wraps agent events and returns content | `wraps_agent_events_and_returns_content` |
| restores agent messages and state after execution | `restores_agent_messages_after_execution` |
| retains agent messages across executions when preserveContext is true | `retains_agent_messages_across_executions_when_preserve_context_is_true` |
| retains appState mutations across executions when preserveContext is true | `retains_app_state_mutations_across_executions_when_preserve_context_is_true` |
| leaves the original printer in place when bufferOutput is not set | not ported — no swappable printer in Rust |
| buffers printer output when bufferOutput is true and flushes on completion | not ported — no swappable printer in Rust |
| passes structuredOutputSchema from options to the agent | `passes_the_structured_output_schema_from_options_to_the_agent` |
| exposes the wrapped agent instance | `exposes_the_wrapped_agent` |
| derives id from orchestrator | `derives_id_and_description_from_the_orchestrator` |
| passes through inner NodeStreamUpdateEvents | `passes_through_inner_node_stream_update_events` |
| wraps non-NodeStreamUpdateEvents with this node identity | `wraps_non_node_stream_update_events_with_this_node_identity` |
| returns orchestrator content | `returns_orchestrator_content` |
| propagates FAILED status from inner orchestrator | `propagates_failed_status_from_the_inner_orchestrator` |
| propagates CANCELLED status from inner orchestrator | `propagates_cancelled_status_from_the_inner_orchestrator` |
| exposes the wrapped orchestrator instance | `derives_id_and_description_from_the_orchestrator` |
| — | `node_definitions_convert_from_every_accepted_shape`, `agent_node_description_defaults_to_the_agents_and_options_override_it` |

| `__tests__/plugins.test.ts` / `__tests__/snapshot.test.ts` `it` | Rust test |
| --- | --- |
| preserves initMultiAgent failures across initialization attempts | `plugins.rs::preserves_init_multi_agent_failures_across_initialization_attempts` |
| — (TS covers duplicates in the agent registry) | `plugins.rs::rejects_a_duplicate_plugin_name` |
| captures orchestratorId, serialized state, and appData | `snapshot.rs::captures_orchestrator_id_serialized_state_and_app_data` |
| omits state when state parameter is undefined | `snapshot.rs::omits_state_when_no_state_is_given` |
| works with Swarm orchestrator | not applicable — `take_snapshot` takes `&dyn MultiAgent` |
| restores MultiAgentState for both Graph and Swarm | `snapshot.rs::restores_multi_agent_state` |
| does not modify state when snapshot has no state data | `snapshot.rs::does_not_modify_state_when_the_snapshot_has_no_state_data` |
| throws on wrong scope | `snapshot.rs::rejects_a_wrong_scope` |
| throws on unsupported schema version | `snapshot.rs::rejects_an_unsupported_schema_version` |
| throws on orchestratorId mismatch | `snapshot.rs::rejects_an_orchestrator_id_mismatch` |
| snapshot survives JSON.stringify/JSON.parse round-trip | `snapshot.rs::survives_a_json_round_trip` |
| — | `snapshot.rs::loads_a_ts_shaped_snapshot_fixture` (TS-shaped JSON fixture) |

`edge.rs::tests` covers the handler/no-handler traversal and the tuple `EdgeDefinition`
conversion (`edge.ts` has no test file of its own).

## Divergences

See the "Multi-agent" decision log in [DESIGN.md](../DESIGN.md). Summary: `ResultStatus` is a
distinct enum; `error` is the serialised message; `interrupts` is a `Vec` with `skip_if_empty`
rather than an `Option`; state types carry no `type` tag (TS omits it too); `nodes` is a
`BTreeMap`; events carry `MultiAgentStateHandle` and do not implement `Serialize`;
`NodeInnerEvent::Custom` is opaque JSON.

## Not ported yet (increment 3+)

`spec.ts` (`MultiAgentSpec` and friends) and `__tests__/spec.test.ts`;
`tests/multiagent_loop.rs`; `tests/integ_multiagent.rs`. Everything else in
`multiagent/{state,multiagent,events,nodes,edge,plugins,snapshot}.ts` is ported.
