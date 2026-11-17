# Strands Agents harness for Rust

`strands-harness` is the Rust port of the Strands Agents harness. `create_harness` builds a ready-to-run agent on the `strands` SDK with the same defaults as the TypeScript harness: built-in file, shell, web and delegation tools, sessions, context management, memory, interventions, skills and MCP servers. It is a developer preview (v0.1).

## Usage

The crate depends on `strands` and runs on `tokio`. Options are built with `HarnessAgentOptions::builder()`; every field has a default, and the result is a plain `strands::Agent`.

```rust
use serde_json::json;
use strands::{InterruptResponse, StopReason};
use strands_harness::{Effort, HarnessAgentOptions, create_harness};

#[tokio::main]
async fn main() -> strands_harness::Result<()> {
    let mut agent = create_harness(
        HarnessAgentOptions::builder()
            .model("anthropic/claude-opus-5")
            .effort(Effort::High)
            .mcp_servers("./mcp.json")
            .interventions("ask")
            .build(),
    )
    .await?;

    let mut result = agent.invoke("Fix the failing test in src/lib.rs").await?;
    while result.stop_reason() == StopReason::Interrupt {
        // Approve every tool call; a real application asks its user.
        let answers: Vec<InterruptResponse> = result
            .interrupts()
            .iter()
            .map(|interrupt| InterruptResponse::new(interrupt.id(), json!("yes")))
            .collect();
        result = agent.invoke(answers).await?;
    }
    agent.shutdown().await?;
    Ok(())
}
```

The options are also a struct with public fields, so a literal over `Default` works. Fields of the SDK's `AgentConfig` that the harness has no option for go in `agent_config`; its `model`, `tools`, `plugins`, `interventions`, `background_tasks` and `context_manager` must stay unset, because the harness option of the same name sets them.

```rust
use strands::AgentConfig;
use strands_harness::{BuiltinToolName, HarnessAgentOptions, MemoryOption, SessionConfig};

let options = HarnessAgentOptions {
    model: Some("anthropic/claude-opus-5".into()),
    instructions: Some("You are a migration assistant.".to_owned()),
    builtin_tools: Some(vec![BuiltinToolName::Read, BuiltinToolName::Shell].into()),
    session: SessionConfig { id: Some("migration".to_owned()), ..Default::default() }.into(),
    memory: MemoryOption::Off,
    agent_config: AgentConfig {
        name: Some("Migrator".to_owned()),
        printer: Some(false),
        ..Default::default()
    },
    ..Default::default()
};
```

Without a `model` the harness uses Bedrock Opus 5 with the default AWS credential chain. The crate documentation lists every option and its default.

### Context management

`context_manager` takes the SDK's strategy type. Unset is the `auto` preset; `.context_manager(ContextManagerPreset::Agentic)` (from `strands::experimental::context_manager`) is the `agentic` preset; `.context_manager(ContextManagerOption::Off)` turns it off.

### Shutting down

Call `agent.shutdown().await` when you are done with an agent. Rust has no async drop, and memory extraction runs in the background after a turn, so without the call the latest turns may not be saved.

## Cargo features

All features are on by default and forward to the `strands` feature of the same name.

| Feature | Enables |
|---|---|
| `bedrock` | `bedrock/...` model strings, bare Bedrock model ids and the default model |
| `anthropic` | `anthropic/...` model strings |
| `openai` | `openai/...` model strings; with `bedrock`, `bedrock-mantle/...` |
| `mcp` | the `mcp_servers` option, and `web_search` through Exa |
| `otel` | the span exporters that `OTEL_TRACES_EXPORTER` names |

With `default-features = false`, a model string of a provider whose feature is off fails with `HarnessError::ModelFeatureDisabled`. That includes the default model without `bedrock`: enable a provider, or pass a model instance. `mcp_servers` and `web_search: WebSearchSetting::Exa` without `mcp` fail with `HarnessError::FeatureDisabled`.

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

## Differences from the TypeScript harness

[docs/DIVERGENCES.md](docs/DIVERGENCES.md) lists every user-visible difference, with the reason. [docs/PARITY.md](docs/PARITY.md) tracks the ported tests.

## Minimum supported Rust version

Rust 1.96.

## Development

See [AGENTS.md](AGENTS.md).
