# `multiagent::graph` port matrix

TS spec: `strands-ts/src/multiagent/graph.ts` (+ `__tests__/graph*.test.ts`,
`__tests__/interrupts.test.ts`). Python tie-breaker: `strands-py/src/strands/multiagent/graph.py`.

## Status

Increment 1 (this branch): the `Graph` handle, `GraphBuilder`, and the full TS constructor
validation, with the `describe('constructor')` tests ported. The execution loop,
`impl MultiAgent for Graph`, `From<Graph> for NodeDefinition`, and the snapshot methods are
increment 2.

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
| `GraphOptions.sessionManager` | — | increment 2; register via `.plugin(..)` |
| `GraphOptions.traceAttributes` | — | increment 2 |
| `graph.id` / `.nodes` / `.edges` / `.config` | `Graph::{id, nodes, edges, config}` | |
| `graph.hookRegistry` | `Graph::hooks()` | |
| `Graph implements MultiAgent` | — | increment 2 |

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
| uses explicit sources instead of auto-detection | `explicit_entry_points_override_auto_detection`, `auto_detects_entry_points_from_nodes_without_incoming_edges` | ported (construction half; execution half in increment 2) |
| everything under `describe('invoke')` / `describe('stream')` | — | increment 2 |
| `graph.invocation-state.test.ts` (all) | — | increment 2 |
| `interrupts.test.ts` (graph half) | — | increment 2 |
| `graph.tracer.test.ts` (all) | — | increment 2 |

Rust tests with no TS counterpart (they cover Rust-only surface):
`takes_node_ids_from_the_nodes_themselves`,
`can_run_concurrently_is_false_for_a_single_linear_path`,
`can_run_concurrently_is_true_when_a_node_fans_out`,
`seeds_the_shared_state_with_every_node_id`, `clones_share_one_state_handle`,
`edge_handlers_read_the_shared_state`, `node_description_falls_back_to_the_agent_description`.
