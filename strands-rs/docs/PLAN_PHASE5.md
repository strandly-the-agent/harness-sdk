# Phase 5 plan — multi-agent, A2A, decisions

Integration branch: `agent-tasks/strands-rs` (fork `strandly-the-agent/harness-sdk`). Phase 4
([PLAN_PHASE4.md](PLAN_PHASE4.md)) runs in parallel on the same integration branch; keep edits to
shared files (`lib.rs`, `Cargo.toml`, `DESIGN.md`, `FEATURE_MATRIX.md`) additive and small so the
merges stay trivial. Rules 1–7 of the Phase 4 plan apply unchanged.

## Workstreams

| Branch suffix     | Scope                                                                                              | TS spec                                                                                                    | Python counterpart                              |
| ----------------- | -------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------- | ----------------------------------------------- |
| `multiagent-core` | `multiagent/{state,nodes,events,edge,plugins,snapshot,spec}` + the `MultiAgent` trait and shared interrupt helpers (`multiagent.ts`) | `strands-ts/src/multiagent/{state,nodes,events,edge,multiagent,plugins,snapshot,spec}.ts` + their tests  | `strands-py/src/strands/multiagent/{base,spec,_snapshot}.py` |
| `graph`           | `Graph` + `GraphBuilder`, graph interrupts, graph tracer, graph invocation-state passthrough        | `strands-ts/src/multiagent/graph.ts`, `__tests__/graph*.test.ts`, graph halves of `interrupts.test.ts`     | `strands-py/src/strands/multiagent/graph.py`    |
| `swarm`           | `Swarm` + `SwarmBuilder`, handoff tool, swarm interrupts, swarm tracer, invocation-state passthrough | `strands-ts/src/multiagent/swarm.ts`, `__tests__/swarm*.test.ts`, swarm halves of `interrupts.test.ts`     | `strands-py/src/strands/multiagent/swarm.py`    |
| `a2a`             | A2A server (`A2AServer`, executor, adapters, events), `A2AAgent` (remote agent as `Agent`-like), `a2a_client` vended tool | `strands-ts/src/a2a/`, `strands-ts/src/vended-tools/a2a-client/`                                           | `strands-py/src/strands/multiagent/a2a`, `agent/a2a_agent.py`, `vended_tools/a2a_client` |
| `decisions`       | `DecisionModel` trait, `LlmDecisionModel`, question/answer types, `decision` span                  | `strands-ts/src/decisions/`                                                                                | — (TS only)                                     |

Order: `multiagent-core`, `a2a`, `decisions` start together. `graph` and `swarm` start once
`multiagent-core` has passed its API review and is merged — both build on its types, and a
consistent API matters more than a day of parallelism.

## Shared API contract (binding for every workstream)

These are the shapes the core workstream must produce and graph/swarm/a2a must consume. The core
builder records the final design (with any refinements) in [DESIGN.md](DESIGN.md) under a
"Multi-agent" heading; graph/swarm follow what DESIGN.md says, not this sketch, if they differ.

- **Module layout.** `strands_agents::multiagent::{state, nodes, events, edge, graph, swarm,
  plugins, snapshot, spec}` with the public types re-exported from `multiagent`;
  `strands_agents::a2a` behind cargo feature `a2a`; `strands_agents::decisions` unconditional.
  `lib.rs` gets one `pub mod` line per module; `prelude` is not extended this phase.
- **Orchestrators are `Clone` handles**, like `Agent` (`Arc<Inner>`, state behind locks, every
  method `&self`). Hooks and nodes receive the orchestrator while it runs; a handle avoids borrow
  conflicts.
- **Invocation API mirrors `Agent`** exactly in names and shapes: `invoke(impl Into<MultiAgentInput>)`,
  `invoke_with(input, MultiAgentInvokeOptions)`, `stream(..)`, `stream_with(..)`; `stream` returns
  a `MultiAgentStream` (`BoxStream<Result<MultiAgentStreamEvent>>` newtype with the same accessor
  style as `AgentStream`). `MultiAgentInvokeOptions` has `invocation_state` and
  `cancellation_token` like `InvokeOptions`.
- **`MultiAgent` trait** is object-safe (`Send + Sync`): `id()`, `name()`, `description()`,
  `hooks()`, plus `invoke_with`/`stream_with` taking the concrete `MultiAgentInput`. The generic
  `invoke(impl Into<..>)` conveniences live on the concrete types (and on a blanket extension trait
  if a `dyn MultiAgent` needs them). `Agent` is *not* a `MultiAgent`; `AgentNode` adapts it.
- **`MultiAgentInput`** is an enum `{ None, Text(String), Blocks(Vec<ContentBlock>),
  InterruptResponses(Vec<InterruptResponseContent>) }` with `From<&str>`, `From<String>`,
  `From<Vec<ContentBlock>>`, `From<Vec<InterruptResponseContent>>`, and `TryFrom<InvokeInput>`
  (rejecting `Messages`/`CheckpointResume`, matching the TS `Exclude<..>`).
- **State types** `Status`, `NodeResult`, `NodeState`, `MultiAgentState`, `MultiAgentResult` are
  plain structs/enums with serde matching TS `toJSON()` byte-for-byte (camelCase, `type` tags
  `nodeResult` / `nodeState` / `multiAgentResult`, SCREAMING status strings, omitted `None`s).
  `MultiAgentResult` implements `Display` (joined text, like `AgentResult`).
- **Nodes.** `trait Node: Send + Sync` (`id()`, `node_type()`, `handle(..)` returning a stream);
  `AgentNode` and `MultiAgentNode` concrete types; a `NodeDefinition` enum with `From<Agent>`,
  `From<Graph>`/`From<Swarm>` (added by those workstreams), `From<Arc<dyn Node>>`, and the
  option structs. Builders accept `impl Into<NodeDefinition>`.
- **Builders.** `Graph::builder()` / `Swarm::builder()` returning `GraphBuilder` / `SwarmBuilder`
  with `.node(id, def)`, `.edge(src, dst)`, `.edge_when(src, dst, handler)`, `.entry_point(id)`,
  `.plugin(..)`, `.hook(..)`, limits via `with_*`-free plain setters (follow `AgentBuilder`), and a
  fallible `build() -> Result<Graph>` that performs the TS constructor validation. No separate
  `GraphConfig` struct in the public API unless `AgentBuilder` has the equivalent.
- **Hooks** reuse `hooks::HookRegistry`; multi-agent events are structs implementing the same
  `HookEvent` trait, registered with `orchestrator.hooks().add_callback(..)`. Stream events are the
  `MultiAgentStreamEvent` enum with the same variant naming scheme as `AgentStreamEvent`.
- **Plugins.** `trait MultiAgentPlugin: Send + Sync { fn name(&self) -> &str; async fn
  init_multi_agent(&self, orchestrator: &dyn MultiAgent) -> Result<()> }` mirroring
  `Plugin::init_agent`; a type may implement both.
- **Snapshots** reuse `session::types::Snapshot`; `Graph::take_snapshot(&self, TakeMultiAgentSnapshotOptions)`
  / `load_snapshot(&self, Snapshot)` mirror `Agent::take_snapshot`/`load_snapshot`. Session-file
  interop with TS/Python is a test.
- **Interrupts.** `InterruptSource::MultiagentHook` already exists; orchestrator-hook interrupts
  are stored on `NodeState.interrupts` and child-agent interrupts forwarded as resume input,
  exactly as `multiagent.ts` does. Resume is `invoke(Vec<InterruptResponseContent>)`.
- **Errors.** No new `Error` variants unless TS adds an error class; validation failures use the
  existing invalid-argument variant with TS-identical messages.
- **Telemetry.** Graph/swarm spans go through `telemetry::` helpers the agent loop already uses;
  attribute names copied from TS.
- **A2A.** No official Rust A2A SDK exists. Evaluate `a2a-rs` (crates.io) against the protocol
  version `@a2a-js/sdk ^0.3.10` speaks; prefer a small in-tree `a2a::types` module (serde structs
  for AgentCard / Message / Part / Task / TaskStatus / JSON-RPC envelope) over a heavy dependency
  (no tonic/gRPC, no websockets). Server over `axum` (feature `a2a` pulls `axum` + `reqwest`);
  client over `reqwest`. Record the decision in DESIGN.md.
- **Decisions.** `trait DecisionModel` with `ask(&self, state: impl Into<DecisionInput>,
  questions: Questions) -> Result<DecisionResult>`; TS's const-generic answer narrowing becomes a
  `Question` enum (`Boolean` / `Choice { choices }`, each with `uncertainty`) and an `Answer` enum
  (`Bool`, `Choice(String)`, `Uncertain(Uncertain)`); `LlmDecisionModel::builder()` mirrors the
  `AgentBuilder` style.

## E2E coverage (required, not optional)

- `tests/multiagent_loop.rs` — graph and swarm driven by `MockMessageModel`: routing, handoffs,
  interrupts + resume, snapshots round-trip, plugin init, cancellation.
- `tests/integ_multiagent.rs` — `#[ignore = "live …"]` graph and swarm on live Bedrock
  (`us.anthropic.claude-haiku-4-5-20251001-v1:0`, `AWS_REGION=us-west-2`), skipping without creds.
- `tests/a2a_e2e.rs` — in-process `A2AServer` on a loopback port, `A2AAgent` + `a2a_client` tool
  against it with `MockMessageModel`; one `#[ignore = "live …"]` variant on Bedrock.
- `tests/decisions_loop.rs` + a live variant for `LlmDecisionModel`.
- Interop fixtures: a TS-produced multi-agent snapshot / `NodeResult` JSON loads in Rust.

## Sandbox notes

2 cores, 8 GB RAM, ~7 GB free disk. Every worktree sources `/tmp/work/env.sh`
(`CARGO_TARGET_DIR=/tmp/target`, `line-tables-only` debug info) so builds share one target
directory; cargo serialises concurrent builds on its lock — that is expected. Worktrees live under
`/tmp/work/wt/<module>`; the main clone is `/tmp/work/harness-sdk`.
