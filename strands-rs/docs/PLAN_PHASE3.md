# Phase 3 plan: SDK feature completion + API hardening

Requested: interrupts/HITL, checkpoints/snapshots of agent state, background tasks (engine),
middleware, agent-as-tool/delegation, MCP (latest protocol via `rmcp`), sandbox, telemetry
(OpenTelemetry), memory manager completion, plus providers (Anthropic, OpenAI) since
`arronbailiss/harness-sdk` has only Bedrock in Rust. Conversation managers are removed (context
managers own that job; `ContextManagerOption::Auto` is the Agent default). Sessions are already
snapshot-based in TS and in our port.

Rules for this phase: every module gets (1) a port with the TS tests mirrored, (2) an e2e/live
test where the feature can be exercised end to end (Bedrock, a local MCP server, an in-memory
OTel exporter, a scripted HITL responder, a local sandbox), (3) an API bar-raiser pass before merge
for anything that adds public surface, (4) a correctness review for loop-touching code.

## Waves

- **S (surveys, parallel):** interrupts+interventions+HITL · middleware+snapshots/checkpoints+
  agent-as-tool/delegation+continuation · MCP (TS client + `rmcp` crate API) + sandbox (TS + the
  Rust `strands-agents/shell` and `strands-agents/box` repos) · telemetry + background-tasks engine
  + memory remainder · Anthropic + OpenAI providers.
- **W1 (SDK, parallel):** middleware · interrupts/HITL · snapshots/checkpoints · telemetry · sandbox.
- **W2 (SDK, parallel):** agent-as-tool/delegation · MCP client · background-tasks engine ·
  memory pass C · Anthropic provider · OpenAI provider.
- **W3 (harness):** interventions option → HITL, `mcp_servers`, telemetry setup, sandbox option,
  subagent via agent-as-tool, resolve_model for anthropic/openai.
- **W4:** reviews (reviewer, API bar-raiser, validator), live e2e, docs, PORT_REPORT.
