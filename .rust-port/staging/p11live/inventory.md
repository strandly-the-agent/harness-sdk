# p11live inventory (Live checklist rows marked `live`, 2026-11-16)

Existing before this track: `tests/bedrock_live.rs` (6), `tests/mantle_live.rs` (9), `tests/anthropic_live.rs` (5, skip: no key), `tests/openai_live.rs` (8, skip: no key). They cover the `models/*` rows.

| TS spec | Rust live tests before | Added by this track |
|---|---|---|
| `agent.test.ts` | `bedrock_live.rs` tool round trip and structured output only | `live_agent.rs` |
| `agent.cancel.test.ts` | none | `live_agent.rs` |
| `agent-as-tool.test.ts` | none | `live_agent.rs` |
| `interrupt.test.ts` | none | `live_agent.rs` |
| `interventions.test.ts` | none | `live_interventions.rs` |
| `cedar.test.node.ts` | none | `live_interventions.rs` |
| `conversation-manager/*` (4 files) | none | `live_context.rs` |
| `mcp/mcp.test.node.ts`, `mcp/mcp-v2.test.node.ts` | none (offline `tests/mcp_stdio.rs` only) | `live_mcp.rs` |
| `models/classifier-strategy.test.ts` | none | `live_routing.rs` |
| `models/openai/mantle-routing.test.node.ts` | none | `live_routing.rs` |
| `skills/agent-skills.test.node.ts` | none | `live_tools.rs` |
| `tools/{bash,http-request,notebook}.test*.ts` | none | `live_tools.rs` |
| `telemetry.test.node.ts` | none | `live_telemetry.rs` |
| `models/{bedrock,anthropic,openai/*}` | existing provider files | unchanged |
