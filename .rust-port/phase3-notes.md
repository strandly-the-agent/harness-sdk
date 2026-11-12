# Phase 3 working notes (implementer)

Design choices (log as P-44+ when committed):
- Loop as an async fn driving an `Emitter` (custom one-slot yielder in `agent/stream.rs`): `emit(event)` parks the loop until the consumer pulls, i.e. TS generator semantics. `AgentStream<'a>` wraps it.
- Live hook events hold `data: XEventData` (exactly the toJSON fields) plus agent view, invocation state/context and control fields. After firing, `data` moves into `AgentStreamEvent::X(data)`.
- Sequential events: `agent: &'a mut LocalAgent`. Per-tool events (BeforeToolCall, AfterToolCall, ToolStreamUpdate, ToolResult): `ToolAgent`.
- Hook callbacks can't fail (TS hooks may throw): `interrupt(params) -> Option<JsonValue>` on BeforeToolCall/BeforeTools records the interrupt in the event; the loop collects after firing; duplicate names -> Error with TS text.
- Per-tool pipeline runs inside the spawned task (hooks fired in the task, records sent through a bounded channel); panics caught with catch_unwind around the tool stream -> error result.
- Continuations (`agent/continuation.ts`) are internal, used only by background tasks: ported with background tasks in Phase 4.
- default-slot: process-global, not ported. queue.ts: only multiagent graph, out of scope.
- AgentConfig keys whose subsystems arrive later (conversationManager, contextManager, plugins, backgroundTasks, interventions, sessionManager, memoryManager, sandbox, storage, traceAttributes, baggageAttributes) are added by those phases.
- Default model: placeholder model whose stream fails with DefaultNotConfiguredError until Phase 5 provides BedrockModel.
- Missing doc comment on #[tool] -> compile error (TS registry rejects empty description).
- New error classes: TypeError (TS TypeError), AgentError? (TS plain Error) -- decide.
