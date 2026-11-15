# Divergences from the TypeScript harness

This file lists every user-visible behaviour difference between the Rust harness (`strands-harness`) and the TypeScript harness, with the reason for it. The TypeScript harness (`harness-ts/`) is the specification, so a difference is recorded here only when Rust can't or shouldn't match it, for example where the language rules out the TypeScript mechanism. Differences in API shape that follow Rust idiom (builders, `Result`, `snake_case` names) aren't listed. A fix never changes a cross-SDK wire or persisted format: data written by either SDK keeps loading in the other.

| Area | TypeScript | Rust | Reason |
|---|---|---|---|
| `ToolAgent::call_tool` | The programmatic tool caller runs only `beforeToolCall` hooks, the tool and `afterToolCall` hooks (no middleware, retry, spans or stream events) | `ToolAgent::call_tool` runs the agent's full per-tool pipeline: `BeforeToolCallEvent` (cancel, rewrite, tool swap, interrupts), the tool, `AfterToolCallEvent` with result rewrite and retry, and `ToolResultEvent`; it returns interrupts and cancels as distinct outcomes and emits no stream records | One pipeline for every tool call (design 8.7); Rust exposes no private registry access |
