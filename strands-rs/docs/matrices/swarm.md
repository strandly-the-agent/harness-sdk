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
| `swarm.invoke(input, options?)` | `Swarm::invoke(impl Into<MultiAgentInput>)`, `MultiAgent::invoke_with` |
| `swarm.stream(input, options?)` | `Swarm::stream(impl Into<MultiAgentInput>)`, `MultiAgent::stream_with` |
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

Deferred to increment 2 (the execution loop): every `it` in `swarm.test.ts` that invokes or
streams the swarm (handoff routing, terminal statuses, `_resolveContent`, context text blocks,
timeouts, external cancellation, hook ordering, `MultiAgentHandoffEvent`, `_findResumeNode`
restore), all of `swarm.invocation-state.test.ts`, the swarm half of `interrupts.test.ts`, and
`swarm.tracer.test.ts`.

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
