# `multiagent::graph` port matrix

TS spec: `strands-ts/src/multiagent/graph.ts` (+ `__tests__/graph*.test.ts`,
`__tests__/interrupts.test.ts`). Python tie-breaker: `strands-py/src/strands/multiagent/graph.py`.

## Status

Increment 1: the `Graph` handle, `GraphBuilder`, and the full TS constructor validation, with the
`describe('constructor')` tests ported. Increment 2 (this branch): the execution loop
(`impl MultiAgent for Graph` + inherent `invoke`/`invoke_with`/`stream`/`stream_with`), hook and
stream events in TS order, limits/cancellation, interrupts (hook gate + child agent) with
same-instance resume, and the snapshot methods. Increment 3 added the telemetry spans
(`invoke_graph <id>` + a `node <id>` child per node execution), `GraphBuilder::trace_attributes`,
and `tests/multiagent_tracer.rs`. Still open: the `session_manager` constructor slot and its
duplicate-plugin check, and the cross-module `tests/multiagent_loop.rs` /
`tests/integ_multiagent.rs` graph halves.

## API

| TS | Rust | Notes |
| --- | --- | --- |
| `new Graph(options)` | `Graph::builder()…build()` | fallible; crate builder convention |
| `GraphConfig.maxConcurrency` | `GraphConfig::max_concurrency: Option<usize>` | `None` == `Infinity` |
| `GraphConfig.maxSteps` | `GraphConfig::max_steps: Option<u64>` | |
| `GraphConfig.timeout` | `GraphConfig::timeout: Option<Duration>` | |
| `GraphConfig.nodeTimeout` | `GraphConfig::node_timeout: Option<Duration>` | |
| `GraphOptions.nodes` | `GraphBuilder::node(def)` | id comes from `Node::id()`, as in TS |
| `GraphOptions.edges` | `GraphBuilder::{edge, edge_when, edge_definition}` | |
| `GraphOptions.sources` | `GraphBuilder::entry_point(id)` | accessor `Graph::sources()` |
| `GraphOptions.plugins` | `GraphBuilder::plugin(..)` | |
| `GraphOptions.sessionManager` | — | not yet; register via `.plugin(..)` |
| `GraphOptions.traceAttributes` | `GraphBuilder::trace_attributes(..)` | merged onto every span the graph emits |
| `graph.id` / `.nodes` / `.edges` / `.config` | `Graph::{id, nodes, edges, config}` | |
| `graph.hookRegistry` | `Graph::hooks()` | |
| `Graph implements MultiAgent` | `impl MultiAgent for Graph` | `invoke_with`/`stream_with`/`as_any` |
| `nodes: [graph]` (`definition instanceof Graph`) | `impl From<Graph> for NodeDefinition` | wraps in a `MultiAgentNode` |
| `graph.invoke(input, options?)` | `Graph::{invoke, invoke_with}` | inherent, `impl Into<MultiAgentInput>` |
| `graph.stream(input, options?)` | `Graph::{stream, stream_with}` | inherent, returns `MultiAgentStream` |
| `graph.initialize()` | `Graph::initialize()` | idempotent; the first invocation calls it |
| (session plugin restore) | `Graph::{take_snapshot, load_snapshot}` | `state: Option<&MultiAgentStateHandle>`; `take_snapshot(None, ..)` uses the last run's state, `load_snapshot(.., None)` makes the next invocation resume |
| `new MultiAgentState` per `_stream` | a fresh `MultiAgentStateHandle` per invocation | concurrent invocations are independent |

## Tests

| TS `it` (`graph.test.ts`) | Rust test | Status |
| --- | --- | --- |
| defaults id to "graph" | `defaults_id_to_graph` | ported |
| accepts a custom id | `accepts_a_custom_id` | ported |
| accepts agent node options | `accepts_agent_node_options` | ported |
| accepts multiAgent node options | `accepts_multi_agent_node_options` | ported (agent node stand-in; nested `Graph` needs `impl MultiAgent`) |
| accepts pre-built Node instances | `accepts_pre_built_node_instances` | ported |
| accepts edge options | `accepts_edge_options` | ported |
| throws on duplicate node IDs | `errors_on_duplicate_node_ids` | ported |
| throws on edge referencing unknown source node | `errors_on_edge_referencing_unknown_source_node` | ported |
| throws on edge referencing unknown target node | `errors_on_edge_referencing_unknown_target_node` | ported |
| throws when graph has no source nodes | `errors_when_graph_has_no_source_nodes` | ported |
| throws on unreachable nodes | `errors_on_unreachable_nodes` | ported |
| throws when explicit source references unknown node | `errors_when_explicit_entry_point_references_unknown_node` | ported |
| throws when maxSteps < 1 | `errors_when_max_steps_is_below_one` | ported |
| throws when maxConcurrency < 1 | `errors_when_max_concurrency_is_below_one` | ported |
| throws when timeout < 1 | `errors_when_timeout_is_below_one` | ported |
| throws when nodeTimeout < 1 | `errors_when_node_timeout_is_below_one` | ported |
| defaults maxConcurrency/maxSteps/timeout/nodeTimeout to Infinity | `defaults_every_limit_to_unbounded` | ported |
| uses explicit sources instead of auto-detection | `explicit_entry_points_override_auto_detection`, `auto_detects_entry_points_from_nodes_without_incoming_edges`, `uses_explicit_entry_points_instead_of_auto_detection` | ported |
| executes linear graph (A -> B -> C) in order | `executes_a_linear_graph_in_order` | ported |
| executes parallel graph (A -> B, A -> C) | `executes_parallel_branches_after_the_source` | ported |
| waits for all dependencies before executing join node | `waits_for_every_dependency_before_a_join_node` | ported (covers "executes join node exactly once") |
| executes nested graph through MultiAgentNode | `runs_a_nested_graph_through_a_multi_agent_node` | ported |
| evaluates conditional edges (+ join-node variants) | `skips_a_conditional_edge_whose_handler_is_false` | ported (false branch; join variants pending) |
| passes task + dependency content to downstream nodes | `passes_the_task_and_dependency_content_to_a_downstream_node` | ported |
| does not pass dependency reasoning blocks downstream | — | pending (`MockMessageModel` cannot emit reasoning blocks) |
| converts ContentBlockData[] input for downstream nodes | — | n/a (Rust input is already `Vec<ContentBlock>`) |
| returns failed result when agent throws | `a_failing_node_lands_as_a_failed_result_and_stops_its_branch` | ported (custom node; the mock model cannot fail) |
| propagates unexpected errors from node execution | — | n/a (`run_node` turns every node error into a `FAILED`/`CANCELLED` result) |
| throws when maxSteps is exceeded | `errors_when_max_steps_is_exceeded` | ported |
| throws when a node exceeds nodeTimeout | `errors_when_a_node_exceeds_the_node_timeout` | ported |
| applies per-node timeout over nodeTimeout | `a_per_node_timeout_overrides_the_graph_node_timeout` | ported |
| does not throw when nodeTimeout is Infinity | covered by every test without `node_timeout` | n/a (`None` == Infinity) |
| per-node timeout of Infinity disables a finite nodeTimeout | `a_per_node_timeout_of_duration_max_disables_the_graph_node_timeout` | ported (`Duration::MAX` is the opt-out) |
| throws when timeout is exceeded | `errors_when_the_wall_clock_timeout_is_exceeded` | ported |
| calls initialize only once across invocations | `initializes_plugins_only_once_across_invocations` | ported |
| respects maxConcurrency limit | `respects_max_concurrency` | ported |
| preserves agent messages and state after execution | — | pending (covered by `nodes.rs` `preserve_context` tests) |
| re-executes node in a cycle when conditional edge allows re-entry | `re_executes_a_node_in_a_cycle_while_the_edge_allows_re_entry` | ported |
| yields lifecycle events in correct order for single node | `yields_lifecycle_events_in_order_for_a_single_node` | ported |
| yields handoff events on transitions between nodes | `yields_a_handoff_event_between_nodes` | ported |
| returns cancelled result when cancel is true / a string | `a_hook_cancels_a_node_with_its_own_message` | ported (string variant) |
| buffers / does not buffer agent printer output | — | n/a (no printer buffering in the Rust port) |
| cleans up running nodes when consumer breaks mid-stream | `dropping_the_stream_cancels_the_invocation` | ported (dropping the stream aborts the invocation task, which drops the node tasks) |
| does not interleave output from parallel nodes on a shared writer | — | n/a (no printer in the Rust port) |
| resumes from the next ready node after a linear graph stops | `resumes_from_the_next_ready_node_after_a_loaded_snapshot` | ported (snapshot instead of session manager) |
| resumes parallel branches independently / respects conditional edges on resume / starts fresh when all completed | — | pending (need the `session_manager` slot) |
| throws when sessionManager is in both constructor arg and plugins | — | not yet (no `session_manager` slot) |
| `graph.invocation-state.test.ts`: forwards invocationState to every node; same reference on every event | `forwards_one_invocation_state_to_every_node_and_event` | ported (both merged) |
| `graph.invocation-state.test.ts`: defaults invocationState to {} | — | n/a (`InvocationState::default()`) |
| `interrupts.test.ts`: Graph agent interrupts and resumes | `resumes_a_child_agent_interrupt_on_the_same_graph` | ported (same-instance resume) |
| `interrupts.test.ts`: BeforeNodeCallEvent.interrupt gates a node, resumes | `a_before_node_call_interrupt_gates_a_node_and_resumes` | ported (also covers "hook-gated node still emits NodeResultEvent and AfterNodeCallEvent") |
| `interrupts.test.ts`: resume against a changed topology throws | `resuming_a_graph_whose_topology_changed_errors` | ported |
| `interrupts.test.ts`: Graph parallel interrupt lets sibling finish; hook gate + tool interrupt across runs; nested orchestrator bubbling | — | pending |
| `graph.tracer.test.ts`: starts and ends multi-agent span on successful invocation | `graph_starts_and_ends_the_multi_agent_span_on_a_successful_invocation` | ported (asserts the exported span, not a tracer spy) |
| `graph.tracer.test.ts`: passes exact usage from result to endMultiAgentSpan | `graph_records_the_exact_result_usage_on_the_multi_agent_span` | ported |
| `graph.tracer.test.ts`: ends multi-agent span with error when maxSteps exceeded | `graph_ends_the_multi_agent_span_with_an_error_when_max_steps_is_exceeded` | ported (span status = error) |
| `graph.tracer.test.ts`: starts and ends node span for each node execution | `graph_starts_and_ends_a_node_span_for_each_node_execution` | ported |
| `graph.tracer.test.ts`: ends node span with COMPLETED status, duration, and zero usage | `graph_ends_a_node_span_with_completed_status_duration_and_usage` | ported (asserts the node's real usage) |
| `graph.tracer.test.ts`: passes exact usage from node result to endNodeSpan | `graph_ends_a_node_span_with_completed_status_duration_and_usage` | ported (merged) |
| `graph.tracer.test.ts`: ends node span with FAILED status when node agent throws | `graph_ends_a_node_span_with_failed_status_when_the_node_agent_errors` | ported |
| `graph.tracer.test.ts`: ends node span with CANCELLED status and zero duration | `graph_ends_a_node_span_with_cancelled_status_and_zero_duration_when_a_hook_cancels` | ported |
| `graph.tracer.test.ts`: ends node span with INTERRUPTED status on a hook interrupt | `graph_ends_a_node_span_with_interrupted_status_when_a_hook_interrupts` | ported |
| `graph.tracer.test.ts`: completes when startMultiAgentSpan / startNodeSpan return null | `telemetry::tracer::tests::ending_an_unstarted_span_is_a_no_op` | adapted — Rust has no nullable span; `SpanHandle::default()` is the no-op |
| `graph.tracer.test.ts`: passes node span to every withSpanContext call | `graph_parents_every_node_span_to_the_multi_agent_span`, `agent_spans_inside_a_node_are_not_parented_to_the_node_span` | adapted — no `withSpanContext`; parenting asserted on the exported spans (divergence) |
| `graph.tracer.test.ts`: creates separate node spans for parallel source nodes | `graph_creates_a_separate_node_span_for_each_parallel_source_node` | ported |
| — | `graph_merges_builder_trace_attributes_onto_its_spans` | Rust-only: `GraphBuilder::trace_attributes` |

Rust tests with no TS counterpart (they cover Rust-only surface):
`takes_node_ids_from_the_nodes_themselves`,
`can_run_concurrently_is_false_for_a_single_linear_path`,
`can_run_concurrently_is_true_when_a_node_fans_out` (both cover the `pub(crate)` predicate),
`seeds_a_fresh_invocation_state_with_every_node_id`,
`concurrent_invocations_each_see_only_their_own_node_results`,
`an_interrupt_leaves_queued_targets_pending`,
`emits_an_after_node_call_event_for_every_node_that_ran`,
`a_node_timeout_error_survives_the_post_loop_drain`,
`edge_handlers_read_the_invocation_state`, `node_description_falls_back_to_the_agent_description`.
