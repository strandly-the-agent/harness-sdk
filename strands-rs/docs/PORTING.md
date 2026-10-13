# Porting guide: TypeScript → Rust

How `strands-ts` constructs map onto `strands-agents`. [DESIGN.md](DESIGN.md) explains *why*; this
is the lookup table for *what*.

| TypeScript | Rust | Notes |
|---|---|---|
| `class XBlock { type = 'xBlock'; toJSON() }` + `XBlockData` | `enum ContentBlock { X(XBlock) }`, `#[serde(rename_all = "camelCase")]` | externally tagged ⇒ `{"x": {...}}`; `Text(String)` ⇒ `{"text": "..."}` |
| `interface Foo { a?: number }` | `struct Foo { a: Option<u64> }` + `#[serde(default, skip_serializing_if = "Option::is_none")]` | absent key, never `null` |
| open string union `'a' \| 'b' \| (string & {})` | `enum { A, B, Other(String) }` with manual serde | `StopReason` |
| `Uint8Array` | `Vec<u8>` with `types::base64_bytes` serde helper | base64 on the wire |
| `JSONValue` / `JSONSchema` | `serde_json::Value` (`types::JsonValue` / `JsonSchema`) | |
| `Record<string, JSONValue>` | `serde_json::Map<String, Value>` | insertion order preserved |
| `abstract class Model<T>` | `trait Model` (object-safe), config on the concrete struct | `Arc<dyn Model>` |
| `AsyncIterable<T>` | `BoxStream<'a, Result<T>>` | `futures::StreamExt` to consume |
| `async function*` yielding events, returning `R` | `BoxStream<Result<Item>>` whose **last** item is `Item::Result(R)` | `AggregatedItem`, `AgentStreamEvent::AgentResult` |
| `AbortSignal` | `tokio_util::sync::CancellationToken` | |
| `class FooError extends Error` | `Error::Foo { .. }` variant (`thiserror`) | `Error::name()` gives the TS class name |
| `error.cause` | `#[source] source: Option<BoxError>` | |
| `Tool.stream(ctx)` generator | `async fn invoke(&self, ctx) -> Result<ToolResultBlock>` + `ctx.progress(v).await` | |
| `tool({ name, inputSchema: z.object(..), callback })` | `FunctionTool::builder(name).typed::<Input>().typed_handler(..)` | `schemars` derives the schema |
| hook `class XEvent` + `addCallback(XEvent, cb)` | `struct XEvent` + `hooks.add_callback::<XEvent>(cb)` | registry keyed by `TypeId` |
| writable event fields (`cancel`, `retry`, …) | `pub` fields on the struct, callbacks get `&mut E` | `Cancel`, `EndTurn` enums replace `boolean \| string` |
| `_shouldReverseCallbacks()` | `HookEvent::reverse_callbacks()` | |
| `agent.messages` (public array) | `agent.messages()` (snapshot) / `agent.with_messages(\|v\| ..)` | handle semantics |
| `agent.state` (`StateStore`) | `agent.state()` | |
| `agent.tool.xxx(args)` proxy | `agent.tool(name).call(args)` / `agent.call_tool(..)` | |
| `InvocationState = Record<string, unknown>` | `InvocationState` (type-erased bag, cloned handle) | identity preserved across events |
| `Plugin` interface | `#[async_trait] trait Plugin` | |
| `logger.warn/debug/...` | `tracing::warn!/debug!/...` | `logging::warn_once` for one-shot warnings |
| `setTimeout` / sleep | `tokio::time::sleep` (injectable in retry) | |
| `crypto.randomUUID()` | `uuid::Uuid::new_v4()` | |
| `string.slice(a, b)` / `.length` | `encode_utf16()`-based helpers where TS semantics matter | never byte-slice user text |
| `JSON.parse` failure (`SyntaxError`) | `serde_json::Error` | |
| `Map` (insertion-ordered) | `indexmap::IndexMap` | `ToolRegistry` |
| `vitest` `describe/it` | `#[cfg(test)] mod tests` + `#[test]`/`#[tokio::test]` | name = TS name in snake_case |
| `MockMessageModel` fixture | `test_support::MockMessageModel` | |

## Gotchas

- TS truthiness: `''`, `0`, `null`, `undefined` are all "unset". When TS does `x || default`,
  port as `if x.is_empty()` / `unwrap_or`, not just `Option`. Example: empty `trackingId` is
  regenerated.
- TS `ToolResultBlock.error` is in-memory only and must never serialize — `#[serde(skip)]`.
- Hook callbacks run **sequentially**; `After*` events run in reverse order. Never parallelize.
- The TS agent loop treats `InterruptError` specially everywhere; interrupts are out of scope but
  `Error::Cancelled` keeps the same "propagate, don't convert to a tool result" asymmetry.
- `MaxTokens` is detected in the model layer (`stream_aggregated`), not the agent loop.
