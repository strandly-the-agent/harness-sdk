# Feature availability: Python / TypeScript / Rust

Where the Rust SDK stands against `strands-py` and `strands-ts`, at a glance. Rows are features,
columns are SDKs. Snapshot of this monorepo at the commit that last touched this file; update it
when a module lands or is dropped.

✅ available · 🟡 partial (see note) · ❌ not available

| Area               | Feature                                                                    | Python | TypeScript | Rust | Notes                                                                                         |
| ------------------ | -------------------------------------------------------------------------- | :----: | :--------: | :--: | --------------------------------------------------------------------------------------------- |
| **Core**           | Agent loop (`invoke` / `stream`, tool calling)                             |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Tool decorator / macro (`@tool`, `tool()`, `#[tool]`)                      |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Tool registry, sequential + concurrent executors                           |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Structured output                                                          |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Hooks (typed event registry)                                               |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Plugins + model plugins                                                    |   ✅   |     ✅     |  ✅  | Python also has entry-point discovery                                                         |
|                    | Middleware stages                                                          |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Retry strategies                                                           |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Agent state store                                                          |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Agent metadata (`id` / `name` / `description`)                             |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Agent-as-tool, delegation                                                  |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Continuations (resume with appended input)                                 |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Interrupts / human-in-the-loop                                             |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Snapshots (`take_snapshot` / `load_snapshot`)                              |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Checkpoints                                                                |   ✅   |     ✅     |  ✅  | experimental in Python/TS                                                                     |
|                    | Background tasks (in-process engine)                                       |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Message injection (`injection/`)                                           |   ✅   |     ✅     |  ✅  | `injection/`; delivered via `BeforeModelCallEvent`, XML helpers shared with `memory/`         |
|                    | Decision model (`decisions/`)                                              |   ❌   |     ✅     |  ❌  |                                                                                               |
|                    | Printer / printing callback handler                                        |   ✅   |     ✅     |  ✅  | `agent::printer` — `AgentPrinter<W: Write>`                                                    |
|                    | Bidirectional streaming (`bidi`)                                           |   ✅   |     ❌     |  ❌  |                                                                                               |
|                    | Tool hot-reload / file loader                                              |   ✅   |     ❌     |  ❌  |                                                                                               |
| **Models**         | Amazon Bedrock (Converse stream, cache points, guardrails config)          |   ✅   |     ✅     |  🟡  | Rust: `AWS_BEARER_TOKEN_BEDROCK` auth not supported; guardrail/throttling paths untested live |
|                    | Anthropic                                                                  |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | OpenAI Chat + Responses                                                    |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Bedrock Mantle (OpenAI-compatible)                                         |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Google Gemini                                                              |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | Vercel AI SDK adapter                                                      |   ❌   |     ✅     |  ❌  |                                                                                               |
|                    | LiteLLM, Ollama, Mistral, Llama API, llama.cpp, SageMaker, Writer          |   ✅   |     ❌     |  ❌  |                                                                                               |
|                    | Model router (fallback / classifier strategies)                            |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | Context-window defaults table                                              |   ✅   |     ✅     |  ✅  |                                                                                               |
| **Context**        | Context manager (`auto` / `agentic`; truncate, summarize, drop, emergency) |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Stash + retrieval tool                                                     |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Conversation managers (sliding window, summarizing, null)                  |   ✅   |     ✅     |  ❌  | Rust dropped them in favour of the context manager                                            |
| **Memory**         | Memory manager + model extractor                                           |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | File memory store                                                          |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Bedrock Knowledge Base memory store                                        |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | Test memory store                                                          |   ✅   |     ✅     |  ❌  |                                                                                               |
| **Storage**        | In-memory storage                                                          |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Local file storage                                                         |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | S3 storage                                                                 |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Storage search                                                             |   ✅   |     ✅     |  🟡  | Rust: keyword only (Python adds BM25, TS adds QMD)                                            |
| **Session**        | Session manager + file storage                                             |   ✅   |     ✅     |  🟡  | Rust: stash / multi-agent snapshot parts not ported                                           |
|                    | S3 session storage                                                         |   ✅   |     ✅     |  ✅  |                                                                                               |
| **Sandbox**        | Local sandbox (process-group shell)                                        |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Docker sandbox                                                             |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | SSH sandbox                                                                |   ✅   |     ✅     |  ❌  |                                                                                               |
| **Interventions**  | Intervention registry + actions                                            |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Human-in-the-loop intervention                                             |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Cedar policy intervention                                                  |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | Steering                                                                   |   ✅   |     ✅     |  ❌  |                                                                                               |
| **Vended plugins** | Skills (progressive disclosure)                                            |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Context injector                                                           |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Context offloader                                                          |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | Goal                                                                       |   ✅   |     ✅     |  ❌  |                                                                                               |
| **Vended tools**   | Shell                                                                      |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | File editor / read / write / edit                                          |   ✅   |     ✅     |  🟡  | Rust: `read`/`write`/`edit` live in `strands-harness`, not the SDK                            |
|                    | Web fetch                                                                  |   ✅   |     ✅     |  🟡  | Rust: `strands-harness` only; host `reqwest`, not sandboxed curl                              |
|                    | Subagent                                                                   |   ✅   |     ✅     |  🟡  | Rust: `strands-harness` only                                                                  |
|                    | HTTP request, Python REPL, notebook, sleep, handoff-to-user                |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | MCP router                                                                 |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | A2A client                                                                 |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | Stop tool                                                                  |   ✅   |     ✅     |  ❌  | experimental in Python/TS                                                                     |
| **MCP**            | MCP client (stdio, streamable HTTP)                                        |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | MCP tasks (long-running), annotations, progress                            |   ✅   |     ✅     |  ❌  |                                                                                               |
| **Multi-agent**    | Graph, Swarm                                                               |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | Multi-agent plugins / snapshots                                            |   ✅   |     ✅     |  ❌  |                                                                                               |
|                    | A2A server + A2A agent                                                     |   ✅   |     ✅     |  ❌  |                                                                                               |
| **Observability**  | Telemetry (`gen_ai.*` spans, events, metrics)                              |   ✅   |     ✅     |  🟡  | Rust: exporters are not installed by the SDK; host wires OTLP                                 |
|                    | Structured logging                                                         |   ✅   |     ✅     |  ✅  | Rust uses `tracing`                                                                           |
| **Harness**        | `create_harness` with TS-parity defaults                                   |   ✅   |     ✅     |  ✅  | `strands-harness` crate                                                                       |
|                    | Built-in tools: shell, read/write/edit, web_fetch, web_search, subagent    |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | `programmatic_tool_caller`                                                 |   ✅   |     ✅     |  ❌  | Needs a Python runtime; Rust omits it with a warning                                          |
|                    | Built-in plugins: todos, environment                                       |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Session, skills, memory, interventions, MCP servers, sandbox options       |   ✅   |     ✅     |  ✅  |                                                                                               |
|                    | Telemetry setup                                                            |   ✅   |     ✅     |  🟡  | Rust: env-gated (`OTEL_TRACES_EXPORTER`) config only                                          |
|                    | Config-file driven agent definition                                        |   ✅   |     ✅     |  ❌  |                                                                                               |

Sources: `strands-py/src/strands`, `strands-ts/src`, `strands-rs/crates/strands-agents/src`,
`strands-rs/crates/strands-harness/src`; Rust gaps per [PORT_REPORT.md](PORT_REPORT.md)
("Not ported" and "Known gaps").
