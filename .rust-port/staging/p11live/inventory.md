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

## Notes while running (2026-11-16)

- `live_summarizes_the_tool_call_in_a_single_summarizer_request` (`live_context.rs`): 2 failures in 32 runs, both on the last assertion (summary text contains `FX-7731`). In both, the summarizer got exactly one request holding the `lookup_order` tool use and its result, and the summary header was `[Summarized: 2 messages`. The model wrote about the tool call as one it should not have made and omitted the result. The request shape matches TS at the pin (same guard and instructions; `noop` tool spec injected when messages hold tool blocks and no specs are given: TS `models/bedrock.ts:967-976`, Rust `models/bedrock/format.rs:329`). The TS test was not run here, so its rate is unknown. Both failures were in whole-file parallel runs (2 of 6); 26 isolated runs passed; the request content is the same in both conditions.
- TS `manager.reduce(..)` and `strategy.apply(..)` are called directly in the TS specs. In Rust their arguments (`ConversationManagerReduceOptions::new`, `LocalAgent::new`) are crate-private, so the live tests drive them through an invocation with a recording model wrapper.
