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

## Tests

`__tests__/state.test.ts` → `state.rs::tests` (NodeResult / NodeState / MultiAgentResult /
MultiAgentState round-trips, status precedence, `_pendingInput` rehydration) and
`multiagent.rs::tests` (response grouping, resume detection).
`__tests__/events.test.ts` → `events.rs::tests` (construction, `_shouldReverseCallbacks` →
`reverse_callbacks`, `toJSON` → `type_name`, `cancel`, hook interrupts).

## Divergences

See the "Multi-agent" decision log in [DESIGN.md](../DESIGN.md). Summary: `ResultStatus` is a
distinct enum; `error` is the serialised message; `interrupts` is a `Vec` with `skip_if_empty`
rather than an `Option`; state types carry no `type` tag (TS omits it too); `nodes` is a
`BTreeMap`; events carry `MultiAgentStateHandle` and do not implement `Serialize`;
`NodeInnerEvent::Custom` is opaque JSON.

## Not ported yet (increment 2+)

`nodes.ts` (`Node` trait, `AgentNode`, `MultiAgentNode`, `NodeDefinition`), `edge.ts`,
`plugins.ts`, `snapshot.ts`, `spec.ts`, and their tests; `tests/multiagent_loop.rs`;
`tests/integ_multiagent.rs`.
