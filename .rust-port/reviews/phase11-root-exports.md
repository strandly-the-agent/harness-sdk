# TS root exports against the Rust crate (track p11misc)

Source: the 387 named exports of `strands-ts/src/index.ts` at the pin (`index.node.ts` re-exports them and registers the Node defaults). Method: each name (also as `snake_case` and `SCREAMING_CASE`) is looked up as a `pub` item in `strands/src`, and its module chain is checked for `pub mod` or a `pub use` in a public ancestor. 253 names resolve to a public path. The crate root holds exactly the section 8.2 list plus the Bedrock types; every other name lives in its module, so no root re-export was added.

## In scope, no Rust counterpart

| TS export | Path it should have | Note |
|---|---|---|
| `LifecycleObserver` (`types/lifecycle-observer.ts`) | `strands::types::lifecycle_observer::LifecycleObserver`, wired where `agent.ts` probes intervention handlers for `observeAgent` | P-413 left it "to telemetry (Phase 10)"; Phase 10 didn't add it. Its other TS users (steering) are out of v0.1. `interventions` is under review, so nothing was added here |
| `SnapshotManifest`, `SnapshotLocation` (`session/types.ts`, `session/storage.ts`) | `strands::session::{SnapshotManifest, SnapshotLocation}` if they are meant to be public | both are `pub(crate)` in `session/snapshot_storage_adapter.rs`; TS exports them for the deprecated `SnapshotStorage` interface, so keeping them private may be the intent. `session` is owned by a fix track |

## Reachable under another name or shape (no action)

- `StreamOptions` is `models::model::ModelRequest`; `ModelStopResponse` is `hooks::ModelStopData`; `ToolUseData` is `types::tools::ToolUse`; `JSONValue` is `JsonValue`; `JSONSchema` is a `JsonValue`; `BedrockCacheTTL` is `BedrockCacheTtl`; `ToolStreamGenerator` is `tools::ToolStream`; `ToolExecutorStrategy` is the `tools::ToolExecutor` enum; `ToolList` is `Vec<ToolSource>`; `ProactiveCompressionConfig` and `ConversationManagerOptions` are `conversation_manager::ProactiveCompression` and the per-manager configs; `RetryStrategy` is the `retry::ModelRetryStrategy` trait (a TS alias); `HookableEvent` and `HookCallback` are the `hooks::HookEvent` trait and closures; `ToolCaller` and `ToolCallerProxy` are `Agent::tool(..)` returning `agent::ToolHandle`; `InvokableTool`, `FunctionToolConfig` and `FunctionToolCallback` are `tools::FunctionTool`; `ZodTool` and `ZodToolConfig` are `#[tool]` with `schemars`; `InterventionActions` is the free functions `interventions::{proceed, deny, guide, confirm, transform}`; `CandidateInput` is `models::routing::CandidateModel`.
- The twelve error classes the lookup missed (`ModelError`, `ContextWindowOverflowError`, the three `Sandbox*Error`s, ...) are macro-generated in `strands::error` and built by `tests/constructors.rs`. `ConcurrentInvocationError` is compile-time (`&mut self`). `InterruptError` is `ToolOutcome::Interrupt` and `MiddlewareError::Interrupt`.
- The `*Data` interfaces of the block, media and stream-event types (31 names besides `ToolUseData`), `contentBlockFromData` and `toolResultContentFromData` are the serde forms of the block types (one Rust type per block).
- The six `Middleware*Handler`/`MiddlewareNextOf`/`MiddlewareInterruptible` aliases are the `middleware::Middleware<S>` closures and the `interrupt()` methods of the stage contexts. `StreamEvent` and `HookableEventConstructor` are TS class plumbing.

## For the lead (modules under review; not checked beyond the name lookup)

- `MemoryStoreConfig` (`memory`): the shared fields are methods of the `memory::MemoryStore` trait; no struct of that name.
- `RoutingCandidateOptions` (`models::routing`): no type of that name; `RoutingCandidate` has builder methods.
- `OffloadStrategyBuilder` (`experimental`): the strategies have inherent `when(..)` methods; no trait of that name.

## Out of v0.1 (section 5.2)

`decisions/` (15 names), `multiagent/` (2) and `MultiAgentSaveLatestStrategy`, the MCP task types (23) and `TasksConfig`, `configureLogging` and `Logger`, the deprecated `FileStorage`, `SessionStorage` and `SnapshotStorage`.
