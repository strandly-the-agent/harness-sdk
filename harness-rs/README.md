# Strands Agents harness for Rust

`strands-harness` is the Rust port of the Strands Agents harness. `create_harness` builds a ready-to-run agent on the `strands` SDK with the same defaults as the TypeScript harness: built-in file, shell, web and delegation tools, sessions, context management, memory, interventions, skills and MCP servers. It is a developer preview (v0.1).

## Telemetry

`create_harness` exports the agent's OpenTelemetry spans when `OTEL_TRACES_EXPORTER` names an exporter (`otlp`, `console`); unset or `none` means off. Endpoint, headers and protocol come from the `OTEL_EXPORTER_OTLP_*` variables.

The OTLP exporter sends spans in batches. The TypeScript harness flushes the last batch when Node exits; a Rust process has no exit hook, so call `shutdown_telemetry` once, before the process exits:

```rust
use strands_harness::{HarnessAgentOptions, create_harness, shutdown_telemetry};

#[tokio::main]
async fn main() -> strands_harness::Result<()> {
    let mut agent = create_harness(HarnessAgentOptions::default()).await?;
    agent.invoke("Hello").await?;
    shutdown_telemetry().await
}
```

It does nothing when no exporter was wired. Agents that run after it export no spans.

## Minimum supported Rust version

Rust 1.96.

## Development

See [AGENTS.md](AGENTS.md).
