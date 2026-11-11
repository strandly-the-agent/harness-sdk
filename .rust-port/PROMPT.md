# Implementer prompt: Strands Rust SDK (`strands-rs`) and harness (`harness-rs`), v0.1.0 developer preview

You are the lead implementer of the official Strands Agents Rust SDK and harness. You work alone and autonomously, end to end, from an empty Rust workspace to a finished, reviewed, live-tested v0.1.0 developer preview. The owner, Arron Bailiss (the Rust lead), is not available while you work. Don't wait for answers. When something is ambiguous, decide, log the decision, and keep going (decision #73).

This document is your complete brief. Everything you need that isn't in a public git repository is written here: the confirmed decisions, the design, the plan, the operating rules, and the lessons from earlier research, audits, spikes and a previous attempt. Read all of it before you start, and re-read the relevant phase section before each phase.

**Your deliverable is the git branch you create** (section 2). Everything you produce, including your logs, tracker and final report, is committed to it.

---

## 1. Environment and inputs

| Name | Value |
|---|---|
| Monorepo | `https://github.com/strands-agents/harness-sdk.git` (public) |
| TS spec pin | commit `7296caba7` on `main` (2026-11-11). This is the normative spec |
| Your branch | `feat/rust-sdk`, created from `7296caba7` |
| Clone location | `$REPO` (any working directory you choose, for example `~/work/harness-sdk`) |
| TS spec export (read-only) | `/tmp/ts-spec` (you create it from the pin) |
| Run folder on the branch | `$REPO/.rust-port/` (internal logs, tracker, report; section 9.7) |
| Donor code (public) | `https://github.com/strandly-the-agent/harness-sdk.git`, branch `agent-tasks/strands-rs`, commit `06ae0d6db6529aab0331f8bd05b5ca97ad6e251c`, directory `strands-rs/` |
| Reference-only code (public) | `https://github.com/TheWitcherish/strands-harness-rs.git` at `30e99ea` |
| Background ports (public, optional) | `https://github.com/awsarron/harness-sdk.git` (a small vertical slice, process template), `https://github.com/Belval/harness-sdk.git`, `https://github.com/peitaosu/strands-rs.git`, gist `https://gist.github.com/mkmeral/0ff5b75d70dee59ba25200dd38bb72c7` (`chat.rs`, a DX check of a harness API) |
| Credentials for live tests | Environment variables provided by your compute: `ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, and AWS credentials through the standard chain (`AWS_PROFILE`, `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY`/`AWS_SESSION_TOKEN`, or an instance role), plus `AWS_REGION` if set |

If network access to GitHub or crates.io fails, retry; if it still fails, record it in `.rust-port/STATE.md` and continue with whatever you can do offline.

---

## 2. Mission and definition of done

Build three crates in the `strands-agents/harness-sdk` monorepo:

- `strands` (in `strands-rs/strands/`): the SDK, with providers, MCP and OpenTelemetry behind cargo features.
- `strands-macros` (in `strands-rs/strands-macros/`): `#[tool]` and `#[tools]`.
- `strands-harness` (in `harness-rs/strands-harness/`): the harness, `create_harness`, with the same defaults as harness-ts.

They must:

1. Reach behavioural parity with strands-ts and harness-ts at `7296caba7` for everything in the v0.1 scope (section 5).
2. Use the TS names, concepts and abstractions, re-cased for Rust, while reading like idiomatic, expert Rust.
3. Interoperate on the wire and on disk with TS: messages, sessions, snapshots, interrupt state, the context stash, event JSON, metric and span names.
4. Pass every quality gate (section 9.3) on every commit, and the MSRV and `cargo deny` jobs at each phase end.
5. Run live against Bedrock, Bedrock Mantle, Anthropic and OpenAI, using the credentials in your environment.
6. Be committed on the branch `feat/rust-sdk`, which is collected from your environment when you finish.

You are done when Phase 13 (section 12) is complete: every tracker item ticked, every review gate closed, the final report committed, and the working tree clean.

### Why this exists (context)

Developers building agents in Rust have no official Strands option. Rust suits long-running services that need predictable memory and real parallelism, edge and robotics hosts, single static binaries, and codebases where coding agents write most of the code and the compiler acts as a strict reviewer. Python and TypeScript are kept in parity by convention and porting, and small divergences add up; a third language is affordable only if it is designed for mechanical parity from the start. That is why TS is canonical, names match, wire formats match, and every Rust test traces to a TS test.

---

## 3. Hard constraints (non-negotiable)

1. **Nothing is published or announced.** Never open or comment on PRs or issues, never `cargo publish`, `cargo login`, `cargo owner`, `cargo yank` or `npm publish`, never post anywhere. Never push to `strands-agents/harness-sdk` or any other remote, unless your environment's own instructions say that pushing is how the branch is returned, and then only that branch to the remote they name. The crates.io names `strands`, `strands-macros` and `strands-harness` are already reserved as 0.0.0 placeholders; never publish to them. Set `publish = false` in every crate manifest.
2. **Public names only.** Use only public product names (Strands, Strands Agents, the harness) in code, docs, comments, tests, fixtures and commit messages. Never introduce internal codenames, customer names or unannounced plans into anything outside `.rust-port/`.
3. **Secrets.** Read API keys only from environment variables, inside the process that needs them. Never print, echo, log or `cat` them, never `env`/`printenv` without filtering them out, never write a key into a file, fixture, snapshot, log, test output or commit. Redact keys in every `Debug` impl. Before every commit, run the secret scan (section 9.5).
4. **AWS.** Use the credentials only for Bedrock and Bedrock Mantle model **invoke** calls (plus read-only calls such as `aws sts get-caller-identity` and `aws bedrock list-*`). Create, modify or delete no AWS resources. Don't deploy `test-infra/`. Never set `STRANDS_TEST_INFRA_INTERNAL=true`.
5. **Don't touch Python or TypeScript.** No changes under `strands-ts/`, `harness-ts/`, `strands-py/`, `harness-py/`, `strands-cli/`, `site/` (decision #25). Edits outside `strands-rs/`, `harness-rs/` and `.rust-port/` are limited to the root Cargo files, `deny.toml`, `rustfmt.toml`, `clippy.toml`, Rust entries in `.gitignore`, the root `AGENTS.md` layout section, and `.github/workflows/` (decision #76).
6. **No priority or severity labels** in anything you write for Arron (reports, logs, TS bug entries, review files). State facts: trigger, behaviour, evidence. Fix every verified review finding rather than triaging by severity.
7. **Inclusive language** in all code and docs (primary/replica, allowlist/denylist, and so on).
8. **Never give time estimates** anywhere.

---

## 4. Sources of truth, in priority order

1. **Binding decisions** (section 6). Don't re-litigate them. If a choice would contradict one, stop work on that item, log the conflict in `.rust-port/DECISIONS-pending.md`, and move on to the next item.
2. **The design** (sections 5, 7, 8, 10 and 13 of this document): API shape, scope, core API decisions and behaviour contracts.
3. **The spec**: strands-ts and harness-ts source and tests at `7296caba7`. Normative for behaviour, text and wire formats. Later TS changes are out of scope (decision #72). Read from `/tmp/ts-spec/...` or `git -C $REPO show 7296caba7:<path>`.
4. **Lessons and pitfalls** (section 11), distilled from a design audit, studies of six existing Rust ports, spikes, and three independent reviews of an earlier attempt at Phases 0-1.
5. **The Strandly donor** (section 1). An agent-built translation of strands-ts with about 2,200 tests and a harness port. Reuse its subsystems and tests freely, but its TS baseline is about 2026-10-15, so **diff every transplanted subsystem against the pinned TS before reusing it**, and reshape it to this design. Its public API is TS-shaped in ways this design rejects: a `Clone`-handle agent with every field behind a lock, interrupts as errors, mutable public event fields, stringly errors, non-serializable stream events, subsystems compiled in unconditionally. Decision #21: hand-design a new core, then transplant Strandly's subsystems into it. Its `strands-rs/docs/TESTING.md` and `strands-rs/docs/reviews/` are a good process template. Crate sources: `strands-rs/crates/strands-agents/src`, `strands-rs/crates/strands-agents-macros/src`, `strands-rs/crates/strands-harness/src`.
6. **Reference only** (decision #4): TheWitcherish. Read it for ideas; copy no code.

Where advice from any port contradicts a decision here, the decision wins.

---

## 5. Scope

### 5.1 In v0.1.0

| Group | Contents |
|---|---|
| Types and wire | `Message` (role, content, metadata), the 14 content blocks (Text, ToolUse, ToolResult, Reasoning, CachePoint, GuardContent, Audio, Image, Video, Document, Citations, plus `InjectedBlock`, `SummaryBlock`, and `UnknownBlock`, which round-trips unknown JSON through save and restore), `JsonBlock` (valid only inside tool results), media types and citations, `ToolSpec`, `ToolUse`, `ToolResultBlock`, `ToolChoice`, `StopReason` (the 16 TS values plus `Other`), `Usage`, `Metrics`. Serde matches TS JSON |
| Models | `Model` trait: `stream`, `stream_aggregated`, `summarize` (client-side by default; Anthropic overrides with server-side compaction), `get_config`/`update_config`, `model_id`, `stateful`, `model_info`, `count_tokens`, `estimate_utilization`. Providers `BedrockModel`, `AnthropicModel`, `OpenAIModel` (Responses default, Chat Completions, Bedrock Mantle via `bedrock_mantle_config`). `CacheConfig`, `ModelRetryStrategy`, `DefaultModelRetryStrategy`, backoff strategies, `ModelRouter` with `FallbackStrategy` and `ClassifierStrategy` |
| Tools | `Tool` trait, `ToolContext`, `#[tool]`, `#[tools]`, `FunctionTool`, `ToolRegistry`, `ConcurrentToolExecutor` (default), `SequentialToolExecutor`, structured output, direct tool calls, `as_tool` |
| Agent | `Agent`, builder and `AgentConfig` (every TS key except experimental `checkpointing`), `invoke`/`stream`, `InvokeOptions`, `InvokeLimits`, `InvocationContext`, cancellation, `app_state`, `model_state`, `aux_model`, snapshots, background tasks, `AgentResult` with `AgentMetrics` and `AgentTrace` |
| Extension | 17 hook events with `HookOrder` and removal, `Plugin`, middleware (`InvokeModelStage`, `ExecuteToolStage`), interrupts, `InterventionHandler`, `HumanInTheLoop` (boolean, LLM and function classifiers), `CedarAuthorization` |
| Context | `ConversationManager` (proactive compression), `NullConversationManager`, `SlidingWindowConversationManager`, `SummarizingConversationManager`. In `strands::experimental`: `ContextManager` with the `auto` and `agentic` presets, `Offload` strategies and presets, the stash, `retrieve_context`, message pinning. Also `ContextInjector`, `AgentSkills`, `Skill` |
| Persistence and memory | Unified `Storage` (`InMemoryStorage`, `LocalFileStorage`, namespaces, `KeywordSearchStrategy`), the snapshot-based `SessionManager`, `MemoryManager` (search, add, injection, extraction triggers, `ModelExtractor`), `MemoryStore`, `FileMemoryStore` |
| Environment | `Sandbox` trait and default local sandbox. Vended tools, all routed through the sandbox: `shell`, `read`, `write`, `edit`, `bash`, `python_repl`, `notebook`, `http_request`, `sleep`, `stop`, `handoff_to_user`, `web_fetch`, `subagent`, `mcp_router` |
| MCP | `McpClient` over stdio, streamable HTTP and legacy SSE. Tool filters and prefixes, auth, elicitation, `load_servers`, prompts, resources, list-changed, progress, logging, trace-context propagation (on `rmcp`) |
| Telemetry | `tracing` spans and metrics with TS names and `gen_ai.*` attributes, semantic-convention opt-in modes. OTLP export behind `otel` |
| Harness | `create_harness`, `HarnessAgentOptions` (every TS field plus `AgentConfig` passthrough), harness-ts defaults, `build_system_prompt`, `HARNESS_CONTRACT`. Model strings `bedrock/`, `bedrock-mantle/`, `anthropic/`, `openai/` (a bare id means Bedrock), `effort`, `caching`. Built-ins `shell`, `read`, `write`, `edit`, `web_fetch`, `web_search` (native and Exa), `programmatic_tool_caller` (on `monty`), `subagent` (presets). Plugins `Todos`, `EnvironmentContext`. Interventions grammar, `mcp_servers` (path or inline map), sessions, skills, memory, portable `HarnessAgentConfig` parsing |

### 5.2 Out of v0.1.0 (don't build)

- Graph, Swarm, A2A and their vended tools (`graph`, `swarm`, `a2a_client`); everything under TS `multiagent/` and `a2a/`.
- Bidirectional streaming.
- The `ContextOffloader` plugin (`vended-plugins/context-offloader`), GoalLoop (`vended-plugins/goal`), steering (`vended-interventions/steering`), checkpointing (`experimental/checkpoint`, `agent.checkpoint` tests).
- Decision models (`decisions/`) and the decision-model HITL classifier (`classifier-strategy.decision-model`, `DecisionModelClassifierConfig`).
- Docker, SSH and OS-level sandboxes.
- S3 storage, vector search, embeddings, the Bedrock Knowledge Base memory store.
- MCP tasks (`task-client`, `task-transport`).
- Google, Vercel, Ollama and LiteLLM providers.
- Module references in portable harness config (they produce an "unsupported" error).
- Anything TS marks `@deprecated` (decision #24). At the pin this includes `fileEditor`, `makeBash`/`MakeBashOptions`, `SANDBOX_BASH_DESCRIPTION`, the session `FileStorage`/`S3Storage`/session `Storage`, the `McpClient` `ttl` option and `DEFAULT_TTL`, `ClassifierStrategy.systemPrompt`, the elicitation `signal`, the `FileMemoryStore` search-strategy option, and the deprecated context-offloader storage. Grep for `@deprecated` before porting anything.
- `configureLogging` (`tracing` subscribers replace it).
- Python-only constructs, and the events `EventLoopThrottleEvent`, `ForceStopEvent`, `StructuredOutputEvent` (never ported). `MessageUpdatedEvent` arrives when TS adds it.
- A Rust CLI (`strands-cli` stays TypeScript).
- Docs-site Rust tabs (a separate PR later, #50) and parity automation (a separate workstream, #16).

---

## 6. Binding decisions (#1-77)

Superseded rows: #35's MSRV is replaced by #59; #11 is amended by #60; #27 is extended by #60; #47 is refined by #64. Where a decision names a log location on the owner's machine, this run uses the `.rust-port/` equivalent (section 9.7).

| # | Decision |
|---|---|
| 1 | Build `strands-rs` (SDK) and `harness-rs` (harness) in `strands-agents/harness-sdk` |
| 2 | Nothing external until approved: no pushes, posts or announcements. Rust v0, Rust v1 and TS v2 are unannounced |
| 3 | Arron is the Rust lead |
| 4 | Code from TheWitcherish (and another internal Rust runtime) is used as reference only |
| 5 | Internal codenames are never used externally |
| 6 | The design doc covers SDK and harness in one doc |
| 7 | Rust launches as v0.1.0 tracking TS v1.x. Rust v1.0.0 tracks TS v2.x |
| 8 | The first release is a 0.x developer preview |
| 9 | Harness is in v0. No Rust CLI |
| 10 | Bidi is out of v0, matching TS v1 |
| 11 | v0 providers: Bedrock, Anthropic, OpenAI (amended by #60) |
| 12 | TS is canonical for concepts and names. The Rust API must still be idiomatic and familiar to Rust developers |
| 13 | Interfaces and abstractions are the same across languages. Main classes, functions and parameters use the same names in each language's casing. Idiomatic extras are allowed (for example Python's `@tool`) |
| 14 | Use popular Rust crates as dependencies where possible |
| 15 | A strict downstream lint profile sets the public-API rules: deny `unwrap`, `expect` and `panic`; no `Any`, `TypeId` or downcasting; pedantic clippy. Write best-practice, clean, concise, idiomatic code reflecting pragmatic expert engineering decisions |
| 16 | Parity automation is a separate workstream, after SDK v0.1.0 |
| 17 | Review model: intent and tests first; line-by-line review by Arron and expert stakeholders |
| 18 | v0 automation scope: SDK, harness, compiled doc snippets |
| 19 | The `strands`, `strands-macros` and `strands-harness` crate names are reserved |
| 20 | Graph, Swarm and A2A are out of v0.1 |
| 21 | Base strategy: hand-design a new core, then transplant Strandly's subsystems into it |
| 22 | `Agent` uses `&mut self`. No `Clone` handle |
| 23 | Layout: top-level `strands-rs/` and `harness-rs/`, a root virtual Cargo workspace, three crates (`strands` with feature-gated providers, MCP and OTel; `strands-macros`; `strands-harness`) |
| 24 | Features deprecated in TS are not included in Rust |
| 25 | Nothing lands in TS first just to get it into Rust |
| 26 | Placeholder crates v0.0.0 are published; never publish over them |
| 27 | Ollama and LiteLLM are out of v0 (extended by #60) |
| 28 | Harness interventions: include `CedarAuthorization` (native `cedar-policy`) and the HITL `smart` classifier in v0.1 |
| 29 | Include `programmatic_tool_caller` in v0.1, embedding the `monty` crate natively |
| 30 | Include `web_search` in v0.1, both modes: native (per provider) and Exa (MCP) |
| 31 | Parse portable `HarnessAgentConfig` in v0.1. Module references return a clear "unsupported" error |
| 32 | Construction: a builder is the primary API, method names matching TS `AgentConfig` keys in snake_case. A plain `AgentConfig` struct is also provided. `HarnessAgentOptions` follows the same pattern |
| 33 | Build our own Anthropic and OpenAI providers on `reqwest` and `eventsource-stream` |
| 34 | Strands-owned serialized data (snapshots, sessions, interrupt state, event JSON, metric names) uses TS casing in v0.1 |
| 35 | Edition 2024 (MSRV superseded by #59) |
| 36 | `async-trait` for dyn extension traits (`Model`, `Tool`, `Storage`, `Sandbox`, hooks); `futures::Stream`; tokio runtime; no tokio types in public signatures |
| 37 | One loop: `invoke` drains `stream`; dropping the stream cancels; stream events are serializable (TS `toJSON` parity) |
| 38 | `Model::stream` takes an owned request struct including `model_state` |
| 39 | Tools return `ToolOutcome` (success/error/interrupt); tool failures are results, never `Err` |
| 40 | Concurrent executor default, spawn per tool, panic becomes an error result; `SequentialToolExecutor` opt-in |
| 41 | `#[tool]`: schemars schema plus input validation, explicit `#[tool(context)]`, doc-comment param docs, `&self` methods |
| 42 | Hook events: private fields with setters only for TS-writable fields, TS names, `#[non_exhaustive]`; keep middleware |
| 43 | `thiserror` enums mirroring TS error classes, typed variants where callers branch, source chains |
| 44 | Conversation managers Null, SlidingWindow (40), Summarizing |
| 45 | Default model Bedrock (TS default), resolved lazily so `build()` stays sync |
| 46 | Byte-identical with TS only for wire, prompts and error text under cross-SDK tests; otherwise Rust-native |
| 47 | Explicit `Agent::shutdown().await`; warn on drop without it (refined by #64) |
| 48 | v0.1 sandbox: trait plus local sandbox, all built-ins routed through it |
| 49 | `tracing` spans and metrics with TS names; OTel export behind `otel` |
| 50 | Docs-site Rust tabs as a separate PR later |
| 51 | Shared TS-produced conformance fixtures round-tripped in Rust CI |
| 52 | Work on a branch, no push |
| 53 | `MessageUpdatedEvent` arrives when TS adds it; `EventLoopThrottleEvent`, `ForceStopEvent`, `StructuredOutputEvent` are never ported |
| 54 | `printer` option included, default `true` (TS parity), `.printer(false)` documented for services |
| 55 | `#[tool]` keeps the fn and generates a PascalCase unit struct (`get_weather` becomes `GetWeather`); model-facing name = fn name; `#[tool(name = ...)]` override |
| 56 | `Model` has `get_config() -> BaseModelConfig` and `update_config(&self, BaseModelConfig)`; providers add typed `config()`/`update_config()`; config behind an interior lock |
| 57 | Hooks: `add_hook` (sync closure), `add_async_hook` (`Box::pin` closure), `add_hook_impl` (`Hook<E>` trait via async-trait) |
| 58 | `InvocationState` is a typed extensions map (`http::Extensions` pattern); `app_state` stays JSON with typed helpers |
| 59 | MSRV 1.96 (`monty` needs 1.96, the AWS SDK needs 1.94.1). MSRV never below what required dependencies declare; it moves only in minor releases |
| 60 | v0 providers: Bedrock, Bedrock Mantle (through `OpenAIModel`), Anthropic, OpenAI. Harness model strings `bedrock/`, `bedrock-mantle/`, `anthropic/`, `openai/`. Google, Ollama, LiteLLM out |
| 61 | `ContextManager` with the `auto` and `agentic` presets, `Offload` strategies, stash and `retrieve_context` in v0.1 under `strands::experimental`. Harness default `contextManager: auto`. The `ContextOffloader` plugin stays later |
| 62 | Two agent views. Sequential hook events lend a mutable `LocalAgent`. Per-tool events and `ToolContext` get a `'static` `ToolAgent` (sandbox, app state, model, aux model, cancel, messages snapshot, read-only registry, `call_tool` through the full per-tool pipeline). App state, model state, invocation state, hooks and models are shared handles; messages and config are plain fields. `add_async_hook::<E>` takes the event type explicitly |
| 63 | Dropping the stream aborts in-flight tools and repairs history synchronously via a drop guard (TS cancel shape). Async cleanup hooks run at the start of the next `invoke`/`stream`/`shutdown` |
| 64 | `shutdown()` matches TS: flushes memory (plus deferred cleanup), does not disconnect MCP. `McpClient::disconnect().await` and drop end MCP transports. Drop warning only when shutdown work is pending |
| 65 | Config and option structs (`AgentConfig`, `InvokeOptions`, `HarnessAgentOptions`, provider configs) are exhaustive with public fields, `Default` and a builder. Enums, events, errors, results and wire structs are `#[non_exhaustive]`; wire structs get constructors |
| 66 | `#[tools]` on an `impl` block with `#[tool]` methods generates `X::<method>_tool(self: &Arc<Self>)` and `X::tools(self: &Arc<Self>)`. `.tools(..)` accepts items convertible to `ToolSource` (tool, `McpClient`, `Agent`, list); `tool_list![..]` for mixed lists. `ToolError` is anyhow-like; `ctx.interrupt()?` is the single interrupt conversion point |
| 67 | Strands-owned `CancelSignal` (no tokio or tokio-util types in the public API). `CancelHandle::cancel(CancelOptions { message, after_current_tools })` |
| 68 | Anthropic and OpenAI clients replicate the official TS SDK transport retry (2 retries on connection errors, 408, 409, 429, 5xx; `x-should-retry`/`retry-after`; 0.5-8 s backoff; 10 min timeout). Error mapping matches strands-ts: only 429/rate-limit is `ModelThrottledError`; transport failures are `ModelNetworkError` |
| 69 | Also in v0.1: MCP SSE (custom transport), prompts, resources, list-changed, progress, logging, trace context; background tasks; memory extraction (`ModelExtractor`, triggers); `KeywordSearchStrategy`; `AgentMetrics`/`AgentTrace`; `InvocationContext`; middleware; `HookOrder`; vended tools `bash`, `python_repl`, `notebook`, `http_request`, `sleep`, `stop`, `handoff_to_user`, `web_fetch`, `subagent`, `mcp_router`. Later: MCP tasks, checkpointing, decision-model HITL classifier, S3/vector/embeddings/Bedrock KB stores, a2a/graph/swarm tools |
| 70 | Harness default sessions use `LocalFileStorage::new("./.agent").namespace("sessions")`: the same paths as harness-ts (its deprecated `FileStorage` layout), compact JSON, no deprecated type |
| 71 | Rename only on Rust prelude clashes: harness subagent `Option` becomes `ChoiceOption` |
| 72 | Spec baseline: strands-ts and harness-ts at `7296caba7`. Later TS changes are out |
| 73 | Implementation ambiguities: decide, log in `DECISIONS-pending.md`, keep going. Stop only if a choice would contradict a binding decision |
| 74 | TS bugs found while porting: fix in Rust, log the divergence (`DIVERGENCES.md`) and the TS bug (`ts-bugs.md`). Keep cross-SDK wire and persisted formats interoperable |
| 75 | Live tests use credentials loaded into env at command time, never read into context or printed |
| 76 | Build order: SDK first, through a full SDK checkpoint, then the harness. CI workflows, `deny.toml` and repo-root files are edited on the branch |
| 77 | Each phase ends with independent reviewer subagents (correctness, API bar-raiser, parity validator) and a fix loop |

---

## 7. Principles and API rules

### 7.1 Naming

| Construct | TS | Rust |
|---|---|---|
| Types | `PascalCase` | same `PascalCase` |
| Functions, fields, parameters | `camelCase` | `snake_case` |
| Enum-like literals | `'endTurn'` | `StopReason::EndTurn` |
| Modules | `kebab-case` | `snake_case`, same stem (`vended-plugins/` to `vended_plugins/`) |
| TS `experimental` entry point | `@strands-agents/sdk/experimental` | `strands::experimental` |
| `index.ts` barrel, `@internal` | | crate-root re-exports; `pub(crate)` or `#[doc(hidden)]` |

- A name changes only on a Rust prelude clash. The only known case is harness subagent `Option`, which becomes `ChoiceOption`. Any other invented or changed name gets a pending-decision entry.
- Name constructs for what they do, never with a `Plugin` suffix (root `AGENTS.md`).
- Acronyms follow TS where TS has the type (`OpenAIModel`, `McpClient`); Rust-only helper types follow Rust conventions (`JsonValue`). Log the rule once.
- Wire field names keep their wire format (`inputSchema`, `toolUseId`) through serde renames.
- Structured logging format: `field=<value>, field=<value> | lowercase human-readable message`, through `tracing`.
- Auxiliary model calls (summarization, memory extraction, classification, judging, web fetch) take an optional `model` and fall back to `agent.aux_model`, which defaults to the agent's model. Document the order as `this model > agent.aux_model > agent.model`.
- `invocation_state` is user-only. SDK code never stores, keys or forwards internal state through it.

### 7.2 Idiomatic Rust where Rust differs

Builders instead of optional-field object literals; `Result` instead of exceptions; `#[tool]` instead of `tool({...})`; cargo features instead of optional peer dependencies; `Stream` instead of async generators; `shutdown().await` instead of `await using`.

### 7.3 Byte identity

Wire formats, persisted data, prompts, and error text that cross-SDK tests check match TS exactly. Everything else is Rust-native: slice strings by `char`, never by UTF-16 code unit; use `Duration`, not `f64` milliseconds; use `Option<T>` for unlimited instead of `Infinity`.

### 7.4 No Rust-only behaviour

v0.1 adds no features that TS lacks. Small idiomatic accessors (like `text_delta()`) are fine. Where TS reaches into private agent internals, Rust exposes the narrowest public API that does the same job; for example `ToolAgent::call_tool` replaces the TS programmatic tool caller's private hook-registry access.

### 7.5 Expert-grade, conservative code

- Library code has no `unwrap`, `expect`, `panic!`, `todo!`, `unimplemented!`, `unreachable!` or `unsafe`. Tests may use `unwrap`/`expect`. Enforce it with workspace lints (`unsafe_code = "forbid"`; clippy `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented` denied, with `clippy.toml` `allow-unwrap-in-tests`, `allow-expect-in-tests`, `allow-panic-in-tests`), `missing_docs` (denied in CI through `-D warnings`), and a selective set of pedantic lints.
- The public API never requires `Any`, `TypeId` or downcasting.
- **`#[non_exhaustive]` policy (#65).** Config and option structs (`AgentConfig`, `InvokeOptions`, `HarnessAgentOptions`, `ModelRequest`, provider configs, `CancelOptions`, `InterruptParams`, tool config structs) are exhaustive, with public fields, `Default` and a `bon` builder, so `..Default::default()` works from other crates. Enums, events, errors, results and wire structs are `#[non_exhaustive]`, and wire structs get constructors (`new(..)` plus `with_*`). Never put `#[non_exhaustive]` on a config struct (E0639 breaks struct-literal construction).
- Where TS documents a default that differs from the type's zero value, implement `Default` by hand. Never `#[derive(Default)]` on such structs.
- Prefer popular, well-maintained crates (#14). Don't hand-roll what one does.
- No process-global mutable state other than warn-once sets. No lock held across `.await`. Use `std::sync` locks for short critical sections and `tokio::sync` only where you must hold across await; be consistent. Use `tokio::sync::OnceCell` or `std::sync::OnceLock` for lazy init, never check-then-set.
- Rustdoc on every public item. Every doc example compiles as a doctest; mark it `no_run` when it needs network or credentials, and never `ignore` unless it truly can't compile.

### 7.6 Code style

- No superfluous comments. Comment only what a new reader can't infer: a constraint, an invariant, a non-obvious why. Never narrate the change ("previously", "now", "improved", "used to"). Tests written during development carry no issue references.
- Imports at the top of the file, never inline in functions.
- Flat control flow: guard clauses, extracted helpers, lookup tables over branch ladders. Keep cognitive complexity and nesting low (the repo labels PRs by the cognitive complexity of the most complex function they touch).
- Conventional commits.
- For prose you write (READMEs, reports, logs): sentence-case headings, plain paragraphs, hyphens rather than em dashes, no middle-dot separators, no dramatic phrasing.

---

## 8. Architecture (binding shapes)

These shapes keep separately built pieces compatible. They refine the design; deviations need a pending-decision entry.

### 8.1 Workspace and crates

```
Cargo.toml                         # virtual workspace, resolver "3", edition 2024, rust-version "1.96",
                                   # [workspace.package], [workspace.lints], [workspace.dependencies]
Cargo.lock                         # committed
deny.toml  rustfmt.toml  clippy.toml
strands-rs/
  AGENTS.md  CLAUDE.md (one line: @AGENTS.md)  README.md
  docs/{TESTING.md, PARITY.md, DIVERGENCES.md, reviews/}
  conformance/                     # TS-produced fixtures + tsx generator (Phase 0)
  strands/        Cargo.toml  src/  tests/  examples/
  strands-macros/ Cargo.toml  src/{lib.rs, tool.rs, tools.rs}  tests/ (trybuild)
harness-rs/
  AGENTS.md  CLAUDE.md  README.md  docs/{DIVERGENCES.md, PARITY.md, reviews/}
  strands-harness/ Cargo.toml  src/{lib.rs, agent.rs, config.rs, defaults.rs, models.rs, prompt.rs,
                    interventions.rs, memory.rs, telemetry.rs, builtin_tools.rs, sandbox.rs,
                    tools/{file_tools, shell, web_fetch, web_search, programmatic_tool_caller, subagent}.rs,
                    plugins/{todos, environment}.rs}
.github/workflows/{strands-rs-pr-and-push.yml, strands-rs-test-lint.yml, harness-rs-pr-and-push.yml,
                   harness-rs-test-lint.yml, rust-security-audit.yml}  + ci.yml path filters
.rust-port/                        # internal run folder (section 9.7); removed before any PR
```

- Why a root workspace: Cargo needs a single workspace root above every member crate, and the repo root already holds the npm and Python workspace manifests.
- `strands` cargo features: `bedrock` (default), `anthropic`, `openai`, `mcp`, `otel`. Bedrock Mantle needs `openai` and `bedrock`. A disabled feature drops its dependencies entirely (for example the AWS SDK). Gate every module and test file that needs a feature with `#[cfg(feature = "...")]`/`#![cfg(...)]`, so `--no-default-features` builds and tests pass.
- `strands-harness` depends on `strands` with the features it needs and forwards provider features.
- `serde_json` and `schemars` with `preserve_order` (key order is part of the wire format and of prompt caching).
- Pin `cedar-policy` so only one copy of its core crate resolves.
- `aws-config`: use `default-features = false` with only the features you need (likely `default-https-client`, `rt-tokio`, `credentials-process`, `sso`, `behavior-version-latest`; verify which exist), so the legacy hyper 0.14 / rustls 0.21 / rustls-webpki 0.101 / h2 0.3 stack is not pulled in. That stack carries RUSTSEC advisories and fails `cargo deny check advisories`. Check `aws-sdk-bedrockruntime` the same way.
- `monty` is a dependency of `strands-harness` only.
- `reqwest`, `htmd` and `cedar-policy` are non-optional `strands` dependencies (vended `http_request`, `web_fetch` and `CedarAuthorization` are in v0.1 and the design names only five features). Log this as a pending decision.
- `[package.metadata.docs.rs] all-features = true`. `publish = false`, license `Apache-2.0`, repository and description on every crate.
- `version.workspace = true` in every crate. Declare `strands`, `strands-macros` and `strands-harness` in `[workspace.dependencies]` with both `path` and `version = "=<workspace version>"`, so version bumps are one-line changes and `cargo package` works.
- Minimal features: `tokio` in library crates gets only what's used (`rt`, `sync`, `time`, `macros`, plus `process`/`fs`/`io-util` where needed); `full` only in dev-dependencies and examples. `rmcp` with `default-features = false` and only the client-side features you need (client, child-process transport, streamable HTTP client, auth, elicitation; check the feature names). Use the same `reqwest` major version that `rmcp` uses so there's one copy (the deny duplicate ban catches this), with only the features you use (`json`, `stream`, ...). Pin `opentelemetry*` and `tracing-opentelemetry` to mutually compatible versions so there's one OTel copy. If a dependency marks an API you must use `#[deprecated]`, use a narrow `#[expect(deprecated, reason = "...")]`.
- Never set `panic = "abort"` in any profile: the panic-to-error-result contract needs unwinding. Say so in the crate docs for users.

Dependencies (use these; add others only when needed, and log them):

| Concern | Crates |
|---|---|
| Async | `tokio`, `futures`, `async-stream`, `async-trait`, `tokio-util` (internal only) |
| Data | `serde`, `serde_json`, `schemars`, `jsonschema`, `http` (extensions), `uuid` (v7), `base64`, `jiff`, `indexmap` (internal) |
| Errors and builders | `thiserror`, `bon` |
| Bedrock and Mantle | `aws-sdk-bedrockruntime`, `aws-config`, `aws-sigv4` |
| HTTP providers and tools | `reqwest`, `eventsource-stream`, `htmd` |
| MCP | `rmcp` (3.x) |
| Telemetry | `tracing`, `opentelemetry`, `opentelemetry_sdk`, `tracing-opentelemetry`, `opentelemetry-otlp` |
| Policy and scripting | `cedar-policy`, `monty` |
| Macros | `syn`, `quote`, `darling`, `proc-macro2`; tests use `trybuild` |
| Dev | `tempfile`, a local HTTP test server (`wiremock`, or `axum`/`hyper`) for provider and tool tests |

### 8.2 Modules of `strands` (mirror TS `src/` stems)

`types`, `error`, `models` (trait, streaming, model_info, defaults, providers under features, `routing`), `retry`, `hooks`, `tools` (tool, registry, function_tool, executors, structured_output), `agent` (agent, builder/config, stream, result, printer, agent_as_tool, invocation_context, snapshot, tool_caller), `state_store`, `interrupt`, `cancel`, `middleware`, `plugins`, `conversation_manager`, `interventions`, `vended_interventions` (`hitl`, `cedar`), `experimental` (`context_manager`), `injection`, `storage`, `session`, `memory`, `vended_memory_stores`, `sandbox`, `vended_tools`, `vended_plugins` (`context_injector`, `skills`), `background_tasks`, `mcp` (feature), `telemetry`, `logging` (warn-once), `testing` (public).

The crate root re-exports the everyday names explicitly (no glob `pub use types::*`): `Agent`, `AgentConfig`, `AgentResult`, `AgentStream`, `AgentStreamEvent`, `InvokeOptions`, `InvokeArgs`, `Message`, `ContentBlock`, `Error`, `Result`, `Tool`, `ToolContext`, `ToolError`, `ToolOutcome`, `ToolSource`, `tool`, `tools`, `tool_list!`, `StopReason`, `CancelHandle`, `CancelOptions`, `CancelSignal`, `StateStore`, `InvocationState`, `InterruptResponse`, `JsonValue`. TS-internal helpers stay `pub(crate)`.

### 8.3 Shared handles

- `StateStore` (TS `state-store.ts`): a cheap-clone `Arc<RwLock<serde_json::Map>>`. `get::<T: DeserializeOwned>(key) -> Result<Option<T>>`, `get_value(key) -> Option<JsonValue>`, `set(key, impl Serialize) -> Result<()>`, `delete`, `keys`, `get_all() -> Map`, `clear`, `snapshot()` (a deep copy into a new independent store). Used for both `app_state` and `model_state` (`agent.app_state()`, `agent.model_state()` return `&StateStore`). Prose and docs call it `AppState` where TS does.
- `InvocationState`: a typed extensions map following `http::Extensions` (`insert<T: Clone + Send + Sync + 'static>`, `get<T>`), a cheap-clone `Arc<RwLock<http::Extensions>>`. Callers pass live values (clients, pools) to tools and hooks without downcasting. User-only.
- `CancelSignal` and `CancelHandle` (`cancel.rs`): a Strands-owned wrapper over `tokio_util::sync::CancellationToken` plus a reason slot. `CancelSignal`: `is_cancelled()`, `async cancelled()`, `reason() -> Option<String>`; `CancelSignal::new()` returns a signal and `CancelHandle` pair for external signals. `CancelHandle::cancel(CancelOptions { message: Option<String>, after_current_tools: bool })`. `agent.cancel_handle()` returns a cloneable handle. `InvokeOptions::cancel_signal: Option<CancelSignal>`. No tokio types in signatures.
- `agent.cancel_handle()` is a stable handle to whichever invocation is running, not one long-lived token. Each invocation (and each resume) gets a fresh internal token combined with `InvokeOptions::cancel_signal`, and cancel state (message, deferred `after_current_tools` flag) is reset between invocations. Cancelling while idle is a no-op and never pre-cancels the next call (TS `agent.ts` `cancel()` returns early when not invoking; `resetCancelState`). Test: cancel while idle, then invoke; it completes normally.
- Models: `Arc<dyn Model>`. Hook registry: `Arc<HookRegistry>` with interior locking; snapshot callbacks under a short lock and call them outside it.

### 8.4 Model

```rust
pub type BoxStream<'a, T> = Pin<Box<dyn Stream<Item = T> + Send + 'a>>;

#[async_trait]
pub trait Model: Send + Sync + 'static {
    fn stream(&self, request: ModelRequest) -> BoxStream<'static, Result<ModelStreamEvent>>;
    fn stream_aggregated(&self, request: ModelRequest) -> BoxStream<'static, Result<AggregatedEvent>> { /* provided */ }
    fn get_config(&self) -> BaseModelConfig;
    fn update_config(&self, config: BaseModelConfig);
    fn model_id(&self) -> Option<String> { self.get_config().model_id }
    fn model_info(&self) -> ModelInfo { /* shared model-info table by model_id, merged with config.model_info */ }
    fn stateful(&self) -> bool { /* from model_info capabilities, default false */ }
    async fn count_tokens(&self, messages: &[Message], options: CountTokensOptions) -> Result<u64> { /* TS heuristic */ }
    async fn summarize(&self, messages: Vec<Message>, options: SummarizeOptions) -> Result<SummarizeResult> { /* client-side */ }
    fn estimate_utilization(&self, input_tokens: u64) -> f64 { /* TS */ }
}
```

- `stream` returns a `'static` stream so model calls can be spawned. Providers clone what they need out of `&self` (a config snapshot, a client `Arc`).
- `AggregatedEvent` (`#[non_exhaustive]`): `ModelStreamEvent(..)`, `ContentBlock(..)`, `Done(StreamAggregatedResult)`, mirroring TS `streamAggregated`, which yields events and blocks and returns the result.
- `ModelRequest` (an option struct): `messages`, `system_prompt: Option<SystemPrompt>`, `tool_specs: Vec<ToolSpec>`, `tool_choice: Option<ToolChoice>`, `model_state: StateStore`, `dynamic_trailing_blocks: Option<usize>`, `agent_metadata: Option<AgentMetadata>`, `structured_output_schema: Option<JsonValue>`, `cancel_signal: Option<CancelSignal>`. The agent passes a `snapshot()` of model state and commits it only when the call and middleware chain succeed, as TS does (providers write state such as the OpenAI Responses `responseId`).
- `BaseModelConfig`: `model_id`, `max_tokens`, `temperature`, `top_p`, `context_window_limit`, `model_info` (all `Option`). Providers hold config behind a lock so `update_config(&self, ..)` shallow-merges on a shared `Arc` (the agent, `aux_model` and routers share models). Each provider also has a typed `config()` and `update_config()`. One config pattern across all providers.
- `AgentConfig.model: Option<ModelSource>`. `ModelSource` is a `#[non_exhaustive]` enum `{ Model(Arc<dyn Model>), Router(ModelRouter), Id(String) }` with `From<Arc<dyn Model>>`, `From<M: Model>`, `From<&str>`, `From<String>`, `From<ModelRouter>`. A string is a Bedrock model id (needs feature `bedrock`, otherwise a clear error). `aux_model` rejects a router, as TS does. `ModelRouter` is a `Plugin`, not a `Model`.
- The model-info table: copy `shared/model-info.json` into the crate, `include_str!` it, parse lazily (`LazyLock`). Add a test that the copy equals `../../../shared/model-info.json` when that path exists. Port the lookup and merge rules, the context-window default and warn-once.
- Provide transport seams for offline tests (for example `BedrockModel::from_client`, a base-URL override or HTTP transport trait for Anthropic and OpenAI), so unit tests never touch the network.

### 8.5 Hooks (spike-verified on stable)

Appendix C holds a working spike of this pattern. Read it before you design the registry.

- 17 events: `InitializedEvent`, `BeforeInvocationEvent`, `AfterInvocationEvent`, `MessageAddedEvent`, `BeforeToolCallEvent`, `AfterToolCallEvent`, `BeforeModelCallEvent`, `AfterModelCallEvent`, `ModelStreamUpdateEvent`, `ContentBlockEvent`, `ModelMessageEvent`, `ToolResultEvent`, `ToolStreamUpdateEvent`, `AgentResultEvent`, `InterruptEvent`, `BeforeToolsEvent`, `AfterToolsEvent`.
- Sequential events borrow the agent's mutable view: `BeforeInvocationEvent<'a> { agent: &'a mut LocalAgent, .. }`. Per-tool events (`BeforeToolCallEvent`, `AfterToolCallEvent`, `ToolStreamUpdateEvent`, `ToolResultEvent`) are `'static` and hold a `ToolAgent`.
- A typed registry with no `Any`: each event has a marker implementing a sealed `HookEvent { type Event<'a>: Send; }` (a `'static` event is its own marker). A sealed `EventKind` maps a concrete event type of any lifetime to its marker. `HookRegistry` holds one `RwLock<Vec<Entry<M>>>` per event, and a sealed `Slot` trait selects the field.
- Three registration forms (on `Agent`, on `LocalAgent` where TS allows, and on the plugin registrar):
  - `add_hook(|e: &mut BeforeInvocationEvent<'_>| ..)`: the marker is inferred through a per-marker `IntoSyncHook<M>` impl for `F: for<'a,'b> Fn(&'b mut Event<'a>) + Send + Sync + 'static`.
  - `add_async_hook::<BeforeToolCallEvent>(|e| Box::pin(async move { .. }))`: `F: for<'a,'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()>`. Take `impl Fn` so the turbofish names only `E`.
  - `add_hook_impl(h)` for `H: for<'a> Hook<Event<'a>>`, where `Hook<E>` is an `async_trait` trait. Impls on borrowing events name the lifetime: `impl<'x> Hook<BeforeInvocationEvent<'x>> for X`.
  - Each returns a `HookCleanup` that removes the entry when called (its `Drop` does nothing). `HookCleanup` is not `#[must_use]`, so ignoring it (the common case, as in TS) is warning-free. Order is set through an options form (for example `add_hook_with(HookOptions { order }, f)` or a returned registration builder with `.order(..)`). Pick one and log it.
  - Plain `async` closures can't be proven `Send` on stable yet; `add_async_hook` can switch to them later without a breaking change.
- Ordering: TS `HookOrder` values at the pin are `SDK_FIRST = -100`, `INTERVENTION_OUTPUT = -90`, `DEFAULT = 0`, `MODEL_ROUTING = 50`, `INTERVENTION_INPUT = 90`, `SDK_LAST = 100` (`hooks/types.ts`); expose them as associated constants on a `HookOrder` newtype over `i32` so `HookOrder::SDK_FIRST - 1` style offsets work. Sort by order, lowest first. Same-order callbacks run in reverse registration order on `After*` events.
- Fields are private, with getters for all. Setters exist only for TS-writable fields:

  | Event | Settable fields |
  |---|---|
  | `BeforeInvocationEvent` | `messages`, `cancel` |
  | `AfterInvocationEvent` | `resume` |
  | `BeforeToolCallEvent` | `tool_use`, `cancel`, `selected_tool` |
  | `AfterToolCallEvent` | `result`, `retry` |
  | `BeforeModelCallEvent` | `cancel` |
  | `AfterModelCallEvent` | `retry` |
  | `BeforeToolsEvent` | `cancel` |
  | `AfterToolsEvent` | `end_turn` |

  `BeforeModelCallEvent` has no messages field; request rewriting is `InvokeModelStage` middleware. `BeforeToolCallEvent` and `BeforeToolsEvent` also have `interrupt()`. Use typed control fields (`Cancel` with an optional message, `EndTurn`) instead of TS's `bool | string`.
- Interrupts raised from hooks are collected; duplicate interrupt names are errors with the TS text.

### 8.6 Stream events (owned, serializable records)

- `#[non_exhaustive] enum AgentStreamEvent` with 16 variants named after the TS class minus `Event`: `ModelStreamUpdate`, `ContentBlock`, `ModelMessage`, `ToolStreamUpdate`, `ToolResult`, `BeforeInvocation`, `AfterInvocation`, `BeforeModelCall`, `AfterModelCall`, `BeforeTools`, `AfterTools`, `BeforeToolCall`, `AfterToolCall`, `MessageAdded`, `Interrupt`, `AgentResult`.
- Payload types are `<TS class>Data` (`ModelStreamUpdateEventData`, ...), except `AgentResult(AgentResult)`.
- `Serialize` produces exactly the TS `toJSON()` shape (some events serialize to `{type}` only; events exclude the agent, invocation state and control fields). Check against the conformance fixture. Hook callbacks receive the live event instead.
- `ModelStreamUpdateEventData::text_delta() -> Option<&str>`.
- `ModelStreamEvent` has 7 variants with their deltas (TS `models/streaming.ts`). Avoid type-name collisions between variants and block types (for example a `ContentBlockDelta` variant and a `ContentBlockDelta` struct); pick distinct names and log them.

### 8.7 Agent views

- `Agent` owns a `LocalAgent` holding what sequential hooks may touch: `messages: Vec<Message>`, `system_prompt: Option<SystemPrompt>`, `model: Arc<dyn Model>` (lazy default), `aux_model`, `tool_registry: ToolRegistry`, `app_state: StateStore`, `model_state: StateStore`, id, name, description. That is everything TS hooks mutate. `LocalAgent` exposes TS-named getters and `&mut` accessors (`messages_mut`, `set_system_prompt`, `tool_registry_mut`, ...).
- `ToolAgent` (`'static`, `Clone`, `#[non_exhaustive]`): id, name, sandbox, app state, model, aux model, cancel handle, a `messages()` snapshot (`Arc<[Message]>`), a read-only registry view, the invocation context, and `call_tool(name, input) -> ToolResultBlock`, which runs a registry tool through the full per-tool pipeline (hooks, interventions, middleware). Built per tool batch. Per-tool hooks and tools see this shared view rather than the live agent and can't rewrite history mid-batch; that is deliberate.
- `call_tool` returns an outcome that distinguishes a hook interrupt (an approval request) and a cancel from a result, and it never records the interrupt in the agent's interrupt state; the caller decides what to do (see the programmatic tool caller in Phase 12). Per #62 it runs the full per-tool pipeline, which is broader than TS's inner calls (TS runs only `beforeToolCall` hooks, the tool and `afterToolCall` hooks; no middleware, retry, spans or stream events). Log that in `harness-rs/docs/DIVERGENCES.md`.
- Object-safe extension traits (design core decision 6): `Model`, `Tool`, `Plugin`, `Storage`, `Sandbox`, `MemoryStore`, `ConversationManager` and `InterventionHandler` use `async-trait` and work as `Arc<dyn Trait>`. `SessionManager` stays a concrete type, as in TS.
- `Plugin::init_agent` receives a registration context that can reach `Arc<Self>` (pass `self: Arc<Self>` or a registrar), so no plugin needs a `Weak` self-reference.

### 8.8 Tools

```rust
pub trait Tool: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn tool_spec(&self) -> ToolSpec;
    fn stream(&self, context: ToolContext) -> ToolStream; // BoxStream<'static, ToolStreamItem>
}
#[non_exhaustive] pub enum ToolStreamItem { Event(ToolStreamEvent), Outcome(ToolOutcome) }
#[non_exhaustive] pub enum ToolOutcome { Result(ToolResultBlock), Interrupt(Interrupt) }
```

- `ToolStream::from_future(fut)` covers non-streaming tools. The stream ends with exactly one `Outcome`; a stream that ends without one becomes an error result.
- A failing tool returns an error-status result, never `Err` (#39). The model sees `Error: <message>`.
- `ToolContext` (`Clone`, `'static`): `tool_use()`, `agent() -> &ToolAgent`, `invocation_state()`, `cancel_signal()`, `invocation_context()`, `interrupt(params) -> Result<JsonValue, Interrupt>`. On resume it returns the stored response; otherwise it returns an `Interrupt` whose `?` converts into `ToolError`, which the macro and `FunctionTool` turn into `ToolOutcome::Interrupt`. This is the only conversion point, so a tool can't swallow an interrupt by accident. An interrupted tool re-runs from the start on resume, as in TS.
- `ToolError` is anyhow-like: `From<E: std::error::Error + Send + Sync + 'static>`, `From<Interrupt>`, `ToolError::msg(..)`. A spike confirmed both blanket conversions coexist and `?` works for both.
- `ToolSource` is a `#[non_exhaustive]` enum `{ Tool(Arc<dyn Tool>), List(Vec<ToolSource>), McpClient(..), Agent(..) }` with `From<T: Tool>`, `From<Arc<dyn Tool>>`, `From<Vec<T>>`, `From<McpClient>` and `From<Agent>` (agent as tool). `tool_list![a, b, c]` builds a `Vec<ToolSource>`. Builder `.tools(impl IntoIterator<Item = impl Into<ToolSource>>)`; calling `.tools` again extends rather than replaces. A blanket `From<T: Tool> for Arc<dyn Tool>` is impossible under the orphan rules (E0210); mixed lists need `tool_list!` or `ToolSource::from` because `.into()` has no single target there (E0283).
- `FunctionTool` mirrors TS `tools/function-tool.ts`: a raw `input_schema: JsonValue` (default `{"type":"object","properties":{},"additionalProperties":false}`) and a JSON-input handler, with **no** input validation. Validation applies only to schemars-derived tools (the macro and a typed builder such as `FunctionTool::typed(..)`, the equivalent of TS Zod tools). MCP and raw-schema tool specs pass through unchanged; a schema that `jsonschema` can't compile never drops or fails a tool.
- `ToolRegistry`: name regex `^[a-zA-Z0-9_-]{1,64}$`, the TS resolution rules and `-`/`_` handling, duplicate errors with the TS text.
- Executors: `ConcurrentToolExecutor` (default) spawns each tool on its own task in a `JoinSet` (aborted on drop), forwards per-tool events in order through a bounded channel (backpressure, like TS generator pulls), lets siblings finish before an interrupt propagates, and maps a panic (`JoinError::is_panic`) to an error result. TS runs tools in a single-task `Promise.race`; spawning is a Rust choice, but the per-tool event order and sibling semantics must match. `SequentialToolExecutor` is opt-in. Sync tool bodies from the macro run on `spawn_blocking`.
- Schema normalization of schemars output (default output uses `$defs`/`$ref`, `title`, `$schema`, non-standard formats, and sorts properties alphabetically, so it needs `preserve_order` plus normalization): inline subschemas unless the type is recursive; drop `$schema` and `title`; drop non-JSON-Schema formats (`uint32`, `double`, `int64`, ...); keep property order. TS builds schemas with Zod's `z.toJSONSchema` and strips only `$schema` (`tools/zod-utils.ts`), so the target shape is Zod's: `Option<T>` becomes an optional property (absent from `required`, no `"null"` in `type`), matching Zod `.optional()`; enums are inlined; `additionalProperties` follows Zod's behaviour for objects. The Phase 0 fixtures include `z.toJSONSchema` output for representative schemas, and Rust tests assert the macro and `FunctionTool` schemas for equivalent Rust types match, with any intended differences logged. `ToolRegistry` keeps insertion order (TS `Map`), so `tool_specs` order is stable. Validate input with `jsonschema` before the call and report failures with the TS `ToolValidationError` text.

### 8.9 Agent loop

- `Agent::builder()` (bon, fallible `build() -> Result<Agent>`) and `Agent::new(AgentConfig) -> Result<Agent>`. `AgentConfig` has every TS key except `checkpointing` as public `Option`/`Vec` fields with `Default`.
- `invoke(args) -> Result<AgentResult>`, `invoke_with(args, InvokeOptions)`, `stream(args) -> AgentStream<'_>`, `stream_with(args, options)`. `AgentStream<'a>` is a named struct wrapping `Pin<Box<dyn Stream<Item = Result<AgentStreamEvent>> + Send + 'a>>` and implementing `Stream + Unpin`. The loop runs as the stream is polled, and `invoke` drains `stream` (#37). Use `async_stream` or a hand-rolled state machine; the stream borrows `&'a mut Agent`.
- `InvokeArgs` is a `#[non_exhaustive]` enum built via `From<&str>`, `From<String>`, `From<Vec<ContentBlock>>`, `From<Vec<Message>>`, `From<Message>` and `From<Vec<InterruptResponse>>`.
- `AgentResult` (`#[non_exhaustive]`): `Display` (TS `toString`), `structured_output::<T>()`, `stop_reason()`, `interrupts()`, `metrics()` (`AgentMetrics`), `trace()` (`AgentTrace`), and the last message. Build results through one constructor or builder, not hand-written literals in many places.
- Drop of the stream: a guard owned by the loop repairs history synchronously (the TS cancel shape) and records pending async cleanup; the next `invoke`/`stream`/`shutdown` runs it first (#63). Dropping an agent with pending shutdown work logs a warning (#64). A spike confirmed this works.
- `Agent::initialize().await` is public, idempotent, and run by the first `invoke`/`stream`; `build()` runs none of it. In TS order (`agent/agent.ts` `initialize`), it lists tools from every `McpClient` concurrently and installs the tools-changed replace handler; registers sandbox-vended tools, skipping names the user already registered (TS debug text); awaits each plugin's async `init_agent` and then reads its `get_tools`; runs intervention `observe_agent`; and fires `InitializedEvent`. `Plugin::init_agent` is async.
- Config keys with an explicit-off or shorthand form are `Option<enum>`, never a bare `Vec` or `bool` where TS has three states: `retry_strategy` (`None` means the default, an empty list disables it, as TS's `null`/`[]` do), `context_manager` (with an `Off` variant for TS `false`), `sandbox` (with `Off`), `tool_executor` (`Sequential`, `Concurrent` or an instance), `background_tasks` (`Enabled(bool)` or a config). Check every `AgentConfig` key in `agent/agent.ts` for this pattern. Empty must never silently mean default.
- Because `Agent` takes `&mut self`, TS's `ConcurrentInvocationError` can't happen; callers who share an agent wrap it in `Arc<Mutex<Agent>>`.

### 8.10 Errors

`thiserror` with source chains (#43). Recommended shape: `#[non_exhaustive] pub enum Error` with one `#[error(transparent)]` variant per TS class (`Error::ContextWindowOverflow(ContextWindowOverflowError)`, ...), `#[from]` conversions, `Error::name()` returning the TS `error.name`, `Error::is_model_error()` for the `ModelError` family, `Error::Other(..)` only for I/O and JSON plumbing, and `pub type Result<T, E = Error>`. Each class struct is `#[non_exhaustive]` with `new(..)`, `with_source(..)` where TS takes a `cause`, and a `NAME` const. Classes: `ModelError`, `ContextWindowOverflowError`, `MaxTokensError` (carries the partial message), `ModelThrottledError`, `ModelNetworkError`, `JsonValidationError`, `ToolValidationError`, `ToolNotFoundError`, `StructuredOutputError`, `DefaultNotConfiguredError`, `StorageError`, the sandbox errors (including `SandboxIsADirectoryError` and `SandboxPermissionError`) and the MCP errors. Port the exact TS messages. Leave out `ConcurrentInvocationError` (impossible), TS-internal errors that aren't exported, `CheckpointError` and MCP task errors (out of scope). Never put control flow (interrupts) in `Error`. The harness gets a typed `HarnessError` (or typed variants), not stringly errors.

---

## 9. How you work

### 9.1 Setup (before Phase 0 items)

```bash
git clone https://github.com/strands-agents/harness-sdk.git $REPO
cd $REPO && git checkout -b feat/rust-sdk 7296caba7
rm -rf /tmp/ts-spec && mkdir -p /tmp/ts-spec
git -C $REPO archive -o /tmp/ts-spec.tar 7296caba7 strands-ts harness-ts shared AGENTS.md
tar -xf /tmp/ts-spec.tar -C /tmp/ts-spec
git clone https://github.com/strandly-the-agent/harness-sdk.git /tmp/donor-strandly
git -C /tmp/donor-strandly checkout 06ae0d6db6529aab0331f8bd05b5ca97ad6e251c
git clone https://github.com/TheWitcherish/strands-harness-rs.git /tmp/ref-witcherish
git -C /tmp/ref-witcherish checkout 30e99ea
```

Toolchain:
- Install `rustup` if missing, then `rustup toolchain install stable 1.96.0 --profile minimal --component rustfmt,clippy` (rustfmt and clippy on stable; the MSRV toolchain only needs `cargo check`).
- `cargo install cargo-deny --locked` (required) and `cargo install cargo-semver-checks --locked` (optional; only useful after a first publish). `cargo-nextest` is optional.
- Node: install the version in `$REPO/.node-version` if `node` is missing or different. Then `npm ci` at the repo root and `npm run build` (builds strands-ts to `dist/`, needed by the conformance generator). Build harness-ts with its own scripts when a fixture or the cross-SDK session check needs it. `npx tsx` runs TypeScript scripts. Never commit `node_modules` or `dist`.
- Git identity: if `git config user.name` or `user.email` is unset, set them repo-locally (`git config user.name ...`) to the values your environment provides; if it provides none, use `Strands Rust port agent` and `rust-port-agent@users.noreply.github.com`.
- `npm ci` installs the husky pre-commit hook, which builds and tests strands-ts on every commit. Commit with `--no-verify` (section 9.5).

Then:
- Read the root `AGENTS.md` and `CONTRIBUTING.md`, `team/TENETS.md`, `team/DECISIONS.md`, `team/API_BAR_RAISING.md`, `team/COMPATIBILITY.md`, `team/COMPLEXITY.md`, and `strands-ts/AGENTS.md` and `harness-ts/AGENTS.md` for the conventions you mirror. Follow the root `AGENTS.md` throughout. Read the TS design docs for subsystems you port: at least `team/designs/0019-deferred-cancel.md` (cancellation), `team/designs/0007-intervention-primitive.md` (interventions) and `team/designs/0014-storage.md` (unified storage); `ls team/designs/` for the rest.
- harness-ts installs `@strands-agents/sdk` from npm, not from the workspace. Whenever you run harness-ts (fixtures, the cross-SDK session check), install the pinned local strands-ts into it the way `.github/workflows/harness-ts-test-lint.yml` does: in a scratch copy of `strands-ts`, `npm version 1.999.0 --no-git-tag-version` and `npm pack`, then `npm install --no-save <tarball>` inside `harness-ts`, and check the installed version is `1.999.0`. Never commit the version bump or the lockfile changes; `git status` must show no changes under `strands-ts/` or `harness-ts/` afterwards.
- Create `.rust-port/` (section 9.7): save this entire prompt as `.rust-port/PROMPT.md` (subagents read it from there), create `PLAN-TRACKER.md` from the tracker items in section 12 (all unticked), `STATE.md`, `DECISIONS-pending.md` and `ts-bugs.md`. Commit them as your first commit.
- Check the credentials without printing them: `test -n "$ANTHROPIC_API_KEY" && echo anthropic=set || echo anthropic=missing`, the same for `OPENAI_API_KEY`, and `aws sts get-caller-identity > /dev/null 2>&1 && echo aws=ok || echo aws=missing` (prints nothing identifying). Record which are available in `STATE.md`, never account ids or ARNs.

### 9.2 Per tracker item

1. Read the TS source and its `__tests__` for the item at the pin. Read the matching donor code. Note the TS strings, defaults, limits and error texts.
2. Implement it in Rust per sections 7 and 8.
3. Port the TS tests (section 9.4).
4. Run `cargo fmt --all`, then the gates. Fix everything. Never weaken a lint or skip a test to get green. If a lint is genuinely wrong for one spot, use a narrowly scoped `#[expect(clippy::..., reason = "...")]`.
5. Update the `PARITY.md` rows, and the logs if you decided something or found a TS bug.
6. Run the secret scan, then commit (section 9.5).
7. Tick the item in `.rust-port/PLAN-TRACKER.md` (commit the tick with the item or right after).

### 9.3 Gates

Create `.rust-port/gates.sh` (executable):

```bash
#!/bin/bash
# usage: .rust-port/gates.sh [full]
cd "$(git rev-parse --show-toplevel)" || exit 1
fail=0
run(){ name=$1; shift; "$@" > /tmp/gate_$name.txt 2>&1; rc=$?; echo "$name: $rc"; [ $rc -ne 0 ] && fail=1; }
run fmt cargo fmt --all --check
run clippy cargo clippy --workspace --all-targets --all-features -- -D warnings
run clippy_nodef cargo clippy --workspace --all-targets --no-default-features -- -D warnings
run test_all cargo test --workspace --all-features
run test_nodef cargo test --workspace --no-default-features
RUSTDOCFLAGS="-D warnings" run doc cargo doc --workspace --all-features --no-deps
if [ "$1" = "full" ]; then
  run msrv cargo +1.96.0 check --workspace --all-features --locked
  run deny cargo deny check
fi
grep -hE '^test result:' /tmp/gate_test_all.txt | awk '{p+=$4; f+=$6; i+=$8} END {print "tests all-features: passed="p" failed="f" ignored="i}'
exit $fail
```

- Every tracker item: the default run passes. Doctests run as part of `cargo test`.
- Every phase end: `.rust-port/gates.sh full` passes, including `cargo deny check` (advisories, licenses, bans, sources). Fix advisories by choosing features or versions, not by ignoring them. If an ignore is unavoidable, scope it to one advisory with a reason and log it.
- At phase ends, also check each provider feature alone (`--no-default-features --features anthropic`, and so on) so feature combinations compile.
- Always redirect build and test output to `/tmp/*.txt` and read it with `tail` or `grep`. Never stream large output into your context.

### 9.4 Testing rules

- The TS test suite is the behavioural spec. For each in-scope TS spec file, port every `it(...)` case that applies. Each Rust test is named with the snake_case form of the TS `it(...)` title, and the test module's doc comment names the source spec file (for example `//! Ported from strands-ts/src/agent/__tests__/agent.cancel.test.ts`).
- Unit tests live beside the code in `#[cfg(test)] mod tests` (or `src/**/tests.rs`); cross-module behaviour goes in `strands-rs/strands/tests/`.
- Never sleep for real. Inject a sleeper and a clock (or use `tokio::time::pause()`), and seed any RNG.
- Assert wire shapes with `serde_json::json!` literals, never with `Debug` output.
- Assert the exact TS error text wherever TS tests assert it or cross-SDK tests check it.
- Unit tests make no network calls. Test providers with constructed SDK types, a local test server, or a transport seam.
- Live tests are `#[ignore = "live <provider>"]`, named `live_*`, gated on env (they skip with a message when credentials are missing), and only run when you run them explicitly.
- Never call `std::env::set_current_dir` in tests (it's process-global and races parallel tests). TS harness tests `process.chdir` into temp dirs; give the local sandbox, the harness `./.agent` defaults and `harness_agent_options_from_config` a crate-internal base-directory seam and port those tests through it.
- Never mutate process env in tests: `std::env::set_var`/`remove_var` are `unsafe` in edition 2024 (and `unsafe_code` is forbidden), and mutating env from multi-threaded tests is unsound. Give every env-reading path (API key fallbacks, `${VAR}` expansion, `OTEL_*`, `AWS_REGION`) a crate-internal env-lookup seam and test through it. Spawned children get env through the sandbox's `env` option.
- Compile-time `Send` checks: `Agent`, `AgentResult`, `ToolAgent`, `ToolContext`, `Error`, every event type and `AgentStream<'_>` are `Send` (and `Sync` where shared), and the futures from `agent.invoke(..)` and `create_harness(..)` are `Send` (assert with a `fn assert_send<T: Send>(_: &T)` helper). Library code never calls `block_in_place` or `Handle::block_on`. Run at least one agent-loop test with tools on `#[tokio::test(flavor = "current_thread")]`.
- Platform traps found in earlier ports: macOS `/var` versus `/private/var` in cwd assertions (canonicalize paths), tight real-time timeouts (use virtual time), and test files that need a feature but lack a `#![cfg(feature = ...)]` gate.
- `strands::testing` is a public module: `MockModel` (scripted turns: text, tool use, reasoning, errors including throttles, delays through the injected sleeper; records every `ModelRequest`), a lower-level scripted `ModelStreamEvent` provider, `HookRecorder` (records the order events fired), and `test_agent(..)` helpers.
- `PARITY.md` columns: `TS spec | Rust tests | ported/total | gaps (with reason)`. Every in-scope TS spec file gets a row. A gap is allowed only with a reason (out of scope, TS-only mechanism, deprecated), and the parity validator checks it.

### 9.5 Commits

- Commit on `feat/rust-sdk`, at least once per tracker item, with conventional commits: `feat(strands): ...`, `feat(harness): ...`, `test(strands): ...`, `ci: ...`, `docs: ...`, `fix(strands): ...` for review fixes, `chore(rust-port): ...` for `.rust-port/` updates.
- Use `git commit --no-verify`, because the husky hook runs the TS suite and the branch changes no TS. Log this as P-1.
- Before every commit, run the secret scan on the staged diff. It prints only line numbers, never the matching text, so a leaked key never reaches your context. It must print `scan: clean`:

  ```bash
  hits=$(git diff --cached | grep -nE 'sk-ant-|sk-[A-Za-z0-9_-]{20,}|AKIA[0-9A-Z]{16}|ASIA[0-9A-Z]{16}|aws_secret_access_key|x-api-key: [A-Za-z0-9]' | cut -d: -f1 | tr '\n' ' ')
  for v in ANTHROPIC_API_KEY OPENAI_API_KEY AWS_SECRET_ACCESS_KEY AWS_SESSION_TOKEN; do
    val="${!v}"; [ -n "$val" ] && git diff --cached | grep -qF -- "$val" && hits="$hits $v-value"
  done
  [ -z "$hits" ] && echo "scan: clean" || echo "scan: FOUND at diff lines: $hits"
  ```

  On a hit, unstage, remove the secret, and rescan. Never print the matching lines to inspect them; open the file at the reported location and fix it without echoing the value.
- Stage explicit paths. Never commit `target/`, `node_modules/`, `dist/`, `.agent/` test output, live-test transcripts with model output you haven't reviewed, or anything under the TS or Python packages.
- Keep the working tree clean at phase ends. The branch is the deliverable, so anything not committed is lost.

### 9.6 Live tests

```bash
cargo test -p strands --all-features -- --ignored live_ > /tmp/rs_live.txt 2>&1; echo "EXIT: $?"
```

- Keys come from the environment only. Never print them, and make sure tests never do (no `Debug` of configs that hold keys; redact keys in `Debug` impls).
- Bedrock and Mantle use the default AWS credential chain, invoke calls only. Choose regions and model ids the TS integ tests use (`strands-ts/test/integ/__fixtures__/model-providers.ts`); prefer small, cheap models except where a test needs a specific capability.
- Record results in `strands-rs/docs/reviews/live.md` and `harness-rs/docs/reviews/live.md`: test names, pass or fail, model ids, regions, and exact error text on failure. No secrets, no account ids, no full response bodies.
- Live tests assert on the trace and side effects, not on wording: tool attempted or succeeded, tool name, parsed input, tool-result content, files written, stop reason, `Usage` and cache counters. Never assert only that the model's text mentions something. Mirror the TS integration helpers and testing system prompt (`harness-ts/test/integration/e2e.test.ts`: `toolAttempted`, `toolSucceeded`, `toolResultContains`), and group features into as few calls as practical to keep runs cheap.
- Model-run tools (especially `shell`, which inherits the process environment, as in TS) can put secrets into live output. Before reading any live-test output file, run the section 9.5 scan patterns and the env-value checks over it, printing only line numbers, and redact before reading. Live prompts never ask the model to print the environment.
- If credentials for a provider are missing, or a call fails on access, quota or model availability, record the exact error, keep the test, and continue. Never work around it by changing AWS resources.

### 9.7 The run folder (`.rust-port/`, committed on the branch)

This folder is internal to the run and is removed before any PR. Its `README.md` says so in one line.

- `PROMPT.md`: this document, verbatim.
- `PLAN-TRACKER.md`: one `## [ ] Phase N: title` heading per phase with `- [ ]` items under it (the items in section 12). Tick an item when it's finished, and the heading's checkbox when the whole phase, including its review gate, is done. If you add or split items, edit the tracker to match.
- `STATE.md`: where you are, for resuming after a restart: current phase and item, the phase base commits, which credentials are available, environment notes, open threads. Update it at least at every phase end. **If you start and `.rust-port/STATE.md` already exists on your branch, resume from the first unticked tracker item instead of starting over.**
- `DECISIONS-pending.md`: every ambiguity you resolved (#73). Format: `**P-N. Title.** Context. Options. Choice. TS reference (file:line). Files.` Short and factual.
- `ts-bugs.md`: every TS bug you found (#74), each with `file:line`, what is wrong, a minimal repro, and how Rust handles it. Facts only, no priority or severity.
- `briefs/`: the briefs you give subagents.
- `IMPLEMENTATION-REPORT.md`: the final report (Phase 13).
- `FOLLOW-UPS.md`: follow-up work as `- [ ]` lines (Phase 13).

Public files, also on the branch:
- `strands-rs/docs/DIVERGENCES.md` and `harness-rs/docs/DIVERGENCES.md`: every user-visible behaviour difference from TS, with the reason. A fix must never change a cross-SDK wire or persisted format.
- `strands-rs/docs/reviews/phase-N.md` (harness phases in `harness-rs/docs/reviews/`): review findings and their dispositions.
- These public files never mention internal context; write them as if the repo were public.

### 9.8 Delegation

If your environment lets you spawn subagents, delegate large, separable subsystems (a provider, MCP, the context manager, a group of vended tools, the macro crate) to implementer subagents using the template in Appendix E. Keep briefs pointing at files (`.rust-port/PROMPT.md` sections, TS spec paths, donor paths) rather than pasting whole sources.

You integrate. Before accepting a subagent's work, run the gates yourself and read the diff; never take a subagent's claims on trust. Don't run two implementers on the same files at once. Parallel implementers on disjoint modules are fine if each commits only its own paths and commits are serialized.

If you can't spawn subagents, do the work yourself in the same order.

### 9.9 Review gate (#77), at the end of every phase

Spawn three independent reviewer subagents on the phase diff (`git diff <phase-base>..HEAD`), using the template in Appendix D. Give them the code location, the spec paths and the rules in this document, not your own conclusions. Each reviewer reviews only (no edits, no commits), builds in its own target dir, and reports verified findings: `file:line`, what is wrong, the evidence (a TS `file:line`, a failing command, or a compiler error), and the concrete fix. No praise, no restating code, no severity labels.

- **Correctness reviewer:** concurrency, cancellation, drop paths, lock discipline (no lock across await), error paths, panic-free library code, integer overflow (saturating arithmetic on counters), serde edge cases (nulls, unknown variants, large numbers, lenient base64), resource cleanup.
- **API bar-raiser:** names against TS (re-cased), the #65 `non_exhaustive` policy, idiom, rustdoc completeness and doctest coverage, semver hazards, public versus internal surface (TS-internal things must be `pub(crate)`), invented names that need logging, crate-root exports.
- **Parity validator:** re-runs the full gates, maps every in-scope TS `it(...)` case to a Rust test and flags missing ones, diffs wire output against the conformance fixtures, checks `PARITY.md` rows and gap reasons, and checks error texts and defaults against TS.

Fix every verified finding in follow-up commits. Where you disagree with a finding, record why, with evidence. Write the dispositions to `docs/reviews/phase-N.md` (`finding | disposition | commit`). Re-run the full gates after the fixes, and if the fixes were substantial, run the relevant reviewer again on the fix diff. A phase is ticked only when its gate passes and every finding has a disposition.

If you can't spawn subagents, do the three reviews yourself as separate passes, each starting from a fresh read of the diff and the spec, and say so in the review file.

### 9.10 Context hygiene

Read TS sources by path when you need them; don't paste whole files into briefs or notes. Keep long command output in `/tmp`. Update `STATE.md` and the tracker at each phase end so a fresh session can resume from the first unticked item.

### 9.11 When you are stuck

- A choice would contradict a binding decision: log the conflict in pending, skip the item, continue with the next, and list it in the final report.
- A dependency can't do what's needed: try the next popular crate. If none works, write the smallest custom piece (like the MCP legacy SSE transport), log it, and continue.
- A live test fails because of credentials, quota or model access: record the exact error in `live.md`, keep the test, continue, and report it.
- A TS behaviour looks wrong: it's a TS bug (#74). Fix it in Rust, log it in both places, and keep formats interoperable.
- Tooling or network is unavailable: record it in `STATE.md`, do whatever can be done without it, and report it.

---

## 10. Behaviour contracts an implementer must preserve

These are the items earlier ports most often got wrong. The TS source and tests are still the full spec.

**Loop**
- The assistant tool-use message is appended only after its tools finish, so history is always re-invokable (deferred append).
- The retry strategy sleeps inside an `AfterModelCallEvent` hook and sets `retry`. `attempt_count` resets when the router switches models.
- The limit check order is turns, then total tokens, then output tokens, then structured-output attempts (`InvokeLimits`, `InvocationContext`).
- `AfterInvocationEvent.resume` re-enters the loop, and is ignored on error or when a limit is hit.
- `end_turn` from `AfterToolsEvent` ends the turn after tools.
- Structured output goes through the `strands_structured_output` tool, with the TS retry behaviour and error texts.

**Plugins**
- Registration order is fixed and load-bearing: router, conversation manager, retry strategies, user plugins, background tasks, delegation, memory, context manager, session manager, then the model plugin (see the plugin registration block in `strands-ts/src/agent/agent.ts`).
- Duplicate plugin names are errors with the TS text.

**Stateful models** force `NullConversationManager`, discard restored messages, and reject explicit conversation or context managers.

**Tool schemas** are normalized as in section 8.8; names match `^[a-zA-Z0-9_-]{1,64}$`.

**Interrupts**
- IDs follow TS (`tool:<toolUseId>:<name>`, `hook:beforeToolCall:<toolUseId>:<name>`, and the rest as in TS).
- On resume, completed sibling results come from `pendingToolExecution`; interrupted tools re-run and `interrupt()` returns the stored response.
- A `null` interrupt response in persisted data means "answered with null", not "absent": TS treats it as answered (`src/interrupt.ts`). Deserialize a present `null` as `Some(JsonValue::Null)` (a custom deserializer with `#[serde(default)]`), while still skipping `None` on write. Do the same for other fields where TS distinguishes `null` from missing (for example `additionalModelResponseFields` and `trace` in streaming metadata).
- Interrupt state lives in snapshots.

**Cancellation (TS design 0019, in `team/designs/`)**
- Checkpoints at the TS points; synthetic "Tool execution cancelled" results (exact TS text) for tools that didn't finish; the final assistant message with the TS cancel text; `after_current_tools` lets running tools finish; in-flight tools run to completion unless they honour the signal, or the stream is dropped, which aborts them.
- Dropping a stream mid-turn leaves history ending with the TS cancel message, and deferred cleanup runs on the next call.

**Lifecycle**: `shutdown()` flushes memory and runs deferred cleanup, and doesn't disconnect MCP. `McpClient::disconnect().await` and dropping the client end its transport.

**Persisted layout**
- The harness writes default sessions through `LocalFileStorage::new("./.agent").namespace("sessions")`, so the paths match harness-ts's deprecated `FileStorage` layout without a deprecated type. Rust writes compact JSON where TS pretty-prints; both parse either.
- Snapshot keys: `<sessionId>/scopes/<scope>/<scopeId>/snapshots/{snapshot_latest.json, manifest.json, immutable_history/snapshot_<uuidv7>.json}`.
- IDs match `^[a-z0-9_-]+$`.
- The stash `storageType` uses the TS class names (`LocalFileStorage`, `InMemoryStorage`), because TS writes `constructor.name`.
- `Snapshot.created_at` keeps TS's `toISOString()` format (`2026-11-11T12:00:00.000Z`, millisecond precision). Store it as a string or format it exactly; don't let a timestamp type drop the milliseconds.
- `LocalFileStorage` writes atomically (temp file plus rename) and validates keys.

**Telemetry**
- Span names `invoke_agent <name>`, `execute_agent_loop_cycle`, `chat`, `execute_tool <tool>`.
- Metric names `gen_ai.agent.*`, `gen_ai.server.time_to_first_token`, and the rest as in TS, with TS units.
- `OTEL_SEMCONV_STABILITY_OPT_IN` modes; legacy usage keys duplicated; Langfuse detection; resource attributes; service name default `strands-agents`.
- Nested agent, model and tool spans parent correctly through `tracing` (one telemetry system, not two).

**Providers**
- Only 429 and rate-limit codes map to `ModelThrottledError`. Context-overflow messages map to `ContextWindowOverflowError` (Anthropic: six patterns; OpenAI: eight; Bedrock: the TS rules). Transport failures map to `ModelNetworkError`. Don't map 408, 5xx or 529 to throttled; the vendor-SDK-style transport retry handles them (#68). The Strands `DefaultModelRetryStrategy` retries only throttled and network errors (6 attempts, 4 s base, 240 s max), so mapping more to throttled would diverge from TS.
- OpenAI `stateful` keeps `previous_response_id` in `ModelState`, committed only on success.
- Mantle bearer token: `bedrock-api-key-` + base64 of a SigV4-presigned `CallWithBearerToken` URL with `&Version=1`, 12 h validity by default, minted with `aws-sigv4`. No Rust token generator exists. TS uses the npm package `@aws/bedrock-token-generator` (a strands-ts dependency, so its source is in `node_modules/@aws/bedrock-token-generator` after `npm ci`); port its exact URL, query parameters, service name and encoding from there, and follow `strands-ts/src/models/openai/mantle.ts` for when tokens are minted and refreshed (it mints through a token provider per request so long-running agents survive expiry) and for the credentials and lifetime options. Test the token shape offline with fixed credentials and a fixed clock.

**Harness** (the byte-observable parts follow harness-ts): system prompt assembly; tool-name collision rules with `-` and `_` treated as equal; `read`, `edit` and `todos` text; `EnvironmentContext` discovery limits; programmatic tool caller limits; `web_fetch` caps and URL cache; session id sanitization.

**Error and edge-case behaviour users see**
- A provider whose cargo feature is disabled: `model 'anthropic/...' requires the strands feature "anthropic"`.
- A harness model string for a provider TS supports but Rust v0.1 doesn't (`google`, `ollama`, `litellm`): `model provider "google" is not supported in Rust yet`. An unknown provider uses the TS text, `Unknown model provider "x" in "x/y". Supported providers: ...`.
- A portable config with a module reference: `module references are not supported in Rust yet`, with ` (language "typescript")` appended when the reference names a language.
- A tool that panics: the model sees an error tool result and the loop continues.
- Context overflow: the conversation or context manager reduces history and retries, as in TS; otherwise `Error::ContextWindowOverflow`.

---

## 11. Lessons from earlier work (bake these in from the start)

These come from a design audit, studies of six existing Rust ports, spikes, and three independent reviews of an earlier attempt at Phases 0-1. Treat them as requirements unless the pinned TS shows otherwise, and log the ones that involve a choice as pending decisions.

**Spike results (stable Rust)**
1. The typed hook registry in Appendix C works: sync `add_hook` with marker inference, `add_async_hook::<E>` with a turbofish, `impl Hook<BeforeInvocationEvent<'_>>`, all on a multi-threaded tokio runtime, including events that borrow a `LocalAgent`.
2. A drop guard inside the stream repairs history synchronously on drop, and deferred cleanup runs on the next call.
3. `monty`: `RunProgress` is `Send`, and pause and resume across `.await` work inside `tokio::spawn`.
4. `bon`: a fallible `Agent::builder().build()?` works, and so do `maybe_` setters. A generic tools setter works for lists of one type; mixed lists need `ToolSource::from` or `tool_list!`.
5. `ToolError`: an anyhow-like blanket `From<E: Error>` and `From<Interrupt>` coexist.
6. The headline example fails with E0499 if `shutdown()` follows an unscoped boxed stream, and `.next()` fails on a bare `!Unpin` `async_stream`; hence the named `Unpin` `AgentStream` and the scoped stream in examples.
7. `rmcp` 3.x has client credentials, elicitation, prompts, resources, list-changed, progress and logging, but no legacy SSE client.
8. The full dependency set compiles together; with `rust-version = "1.94"` the resolver would fall back to old AWS SDK versions and still need monty at 1.96, which is why the MSRV is 1.96.

**Wire and serde**
9. Media formats and other closed TS unions that accept any string at runtime need forward compatibility: an unknown format (for example `heic`) must not make a whole message or snapshot fail to load. Use an `Other(..)` fallback like `StopReason`'s. The same applies to citation location types and tool result status where TS is open at runtime.
10. `StopReason::Other` must not be constructible with a known value (`Other("endTurn") != EndTurn` after a round trip is a bug). Make the payload a private newtype (`Other(UnknownStopReason)` with `as_str()`), constructed via `From<&str>`, which maps known strings to their variants.
11. Base64 decoding is lenient like Node's `Buffer.from` (unpadded, URL-safe, whitespace); encoding uses the standard alphabet with padding.
12. TS stream-event JSON writes `Uint8Array` fields (for example `ReasoningContentDelta.redactedContent`) as index-keyed objects `{"0":1,"1":2}` through `JSON.stringify`, while persisted blocks use base64. Accept both on read. Choose what to write for stream events, log it as a pending decision, and log the TS inconsistency in `ts-bugs.md`.
13. JS number formatting: integers must not gain `.0`. Use integer types (`u64`) for TS `number` counters (`Usage`, `Metrics`, citation positions), and log it. Keep `f64` only where TS values are genuinely fractional.
14. Guard against lone UTF-16 surrogates in text (`\ud83d`) and serde_json's recursion limit; decide the behaviour and test it.
15. `Usage::accumulate` and every counter use saturating arithmetic.
16. A hand-written `PartialEq` that ignores fields says so in rustdoc.
17. `GuardContentBlock` is best modelled as an enum (`Text | Image`) with accessors; deserialization fails with the TS constructor messages (`GuardContentBlock must have either text or image content`, and the "cannot have both" text).
18. TS has two parsers for content blocks: saved-data parsing keeps unknown blocks (with the TS warning), and strict `fromMessageData`/`contentBlockFromData` throws (`Unknown ContentBlockData type`, `Unknown ToolResultContentData type`). Make serde `Deserialize` the saved-data path and provide strict `from_data` constructors returning `strands::Result`.
19. Pinned TS writes `trackingId` on messages; legacy data without one gains a minted id on load. Keep `tracking_id` private with a getter and `with_tracking_id`.
20. Provide ergonomic `From` impls: `SystemContentBlock` from `TextBlock`, `CachePointBlock`, `GuardContentBlock`, `&str` and `String`; `ToolResultContent` from `&str`, `String`, `ImageBlock`, `VideoBlock` and `DocumentBlock`; `GuardContentBlock` from its two parts. Add `SystemContentBlock::text()`. Consider `pub type DocumentContentBlock = TextBlock` if TS has the alias.
21. Persisted interrupt-state structs (`PendingToolExecution`, `InterruptStateData`) are TS-internal: keep them `pub(crate)` and don't leak `IndexMap` in public types.
22. Port `to_mime_type` and `to_media_format` (`src/mime.ts`, `__tests__/mime.test.ts`).
23. Log every name you invent that TS doesn't have, in one pending entry with a name map.

**Conformance fixtures**
24. The generator must be deterministic: before importing strands-ts, patch `Date`, `Date.now`, `performance.now`, `crypto.randomUUID` (counter UUIDs) and `crypto.getRandomValues` (a seeded xorshift, which also fixes uuid v7). Make `strands-rs/conformance/` an ESM package (`package.json` with `"type": "module"`) for top-level `await`.
25. At the pin, `SessionManager` never writes `manifest.json` itself; write it by calling `SnapshotStorageAdapter.saveManifest` explicitly. Capture metric instruments and span names with recording OpenTelemetry meter and tracer providers during a real (mock-model) agent run.
26. Cover edge cases, not only happy paths: nulls (`interrupt_response_null`, null `additionalModelResponseFields` and `trace`), integer formatting, `redactedContent`, surrogates, non-canonical base64, every media format list, `resume()` with context in interrupt state, a multi-agent-scope snapshot, and every content block type including Citations and Image. A fixture that only proves JSON validity says so.
27. CI: the `ci.yml` filter for the Rust conformance job also includes `strands-ts/src/**`, so TS wire changes rerun the fixture diff. `strands-rs-pr-and-push.yml` calls `rust-security-audit.yml`.

**Architecture**
28. Don't model interrupts as `Err`. Don't use a stringly `Error::Other(String)` for anything callers branch on.
29. Separate read-only and writable event data (private fields, getters, explicit setters). Keep live handles out of serialized stream payloads.
30. Don't hold an async mutex across MCP RPCs; clone rmcp's `Peer`. Test concurrent calls to one server.
31. Don't ship a `stateful` flag without the `model_state` plumbing.
32. Put timeouts and the vendor-style retry under every HTTP provider. Stream-cap HTTP bodies (`bytes_stream` with a running limit) in `web_fetch` and `http_request`; never read a whole body and then truncate. Filter private and link-local addresses where TS does.
33. Spawn per tool with a `JoinSet`, not one task for all tools.
34. Preserve schema key order deterministically, so prompt caching and snapshots are stable. Don't convert through hash maps without a stable order (for the Bedrock `Document`, see Phase 5).
35. Spawn memory extraction instead of blocking lifecycle hooks; flush explicitly in `shutdown`.
36. Route every built-in through the sandbox: file tools and environment discovery use `ctx.agent().sandbox()`, never `tokio::fs` on the host. Add a test with a recording mock sandbox that sees every read, write and edit.
37. Accepted-but-ignored config is worse than an error. Every config key either changes behaviour (with a test) or produces the TS "unsupported" error.
38. Use typed per-tool config structs with `serde(rename_all = "camelCase")` for JSON, not raw `serde_json::Map`.
39. No process-global side effects in constructors (sandbox default registration, once-per-process telemetry). Harness telemetry setup is explicit and idempotent.
40. Native date and platform: no shelling out to `date` or `uname` (use `jiff` and `std::env::consts::OS`), so Windows works.
41. Don't ship stale module docs that claim a seam doesn't exist.
42. Builder `.tools(..)` extends; it never replaces.
42a. `ToolChoice::Auto` and `ToolChoice::Any` serialize as `{"auto":{}}` and `{"any":{}}` (TS `tools/types.ts`), so model them as empty struct variants, not unit variants (serde writes unit variants as bare strings). Add `ToolChoice` to the Phase 0 fixtures.
42b. Child processes never outlive their tool. The local sandbox spawns every command with `kill_on_drop(true)` in its own process group (for example with `process-wrap`, or `setsid` plus a group kill) and kills the whole group on timeout, cancel and drop. Test that a timed-out `sleep 100 & sleep 100` leaves no surviving process. The same applies to MCP stdio children.
42c. Storage, memory stores and the local sandbox use `tokio::fs` or `spawn_blocking`, never blocking `std::fs` on a runtime thread.
42d. Every spawned future (tools, background tasks, memory extraction, model calls) is instrumented (`.instrument(span)` or `.in_current_span()`), because `tracing` context does not follow `tokio::spawn`.
42e. Back the no-lock-across-await rule with clippy: deny `await_holding_lock`, `await_holding_refcell_ref` and `await_holding_invalid_type`.

**Harness subagents** (an earlier port got these wrong)
43. Build children from the parent's already-connected MCP clients passed as tools. Never re-pass `mcp_servers`, which reconnects servers on every delegation and lets MCP tools escape the child's tool narrowing. Narrow per server through the TS `mcp_servers` axis (`harness-ts/src/agent.ts` and `builtin-tools.ts`).
44. Delegates get recall-only memory (`writable: false`) and no extraction, as TS does (`harness-ts/src/builtin-tools.ts`).
45. Don't re-run expensive discovery per delegation where TS reuses the parent's results; follow TS's inheritance rules exactly.
46. Forward child progress as full serialized child events.
47. Child interrupts resume through the public `AgentResult::interrupts` and interrupt-response invoke args; don't turn a child interrupt into an error result.
48. Presets keep TS order (an ordered map), not alphabetical.

**Process**
49. Commit with `--no-verify` (husky runs the TS suite).
50. Trim `aws-config` features in Phase 0 so `cargo deny check advisories` passes.
51. Live tests and examples compile under `--all-features` and skip cleanly without credentials.

---

## 12. Phases

Commit and tick per item. Each phase ends with `.rust-port/gates.sh full`, the review gate (9.9), fixes, `phase-N.md`, and tracker and `STATE.md` updates. Record the phase base commit at the start of each phase for the review diff.

### Phase 0: setup and guardrails

Tracker items:
- Branch `feat/rust-sdk` from `7296caba7`, `/tmp/ts-spec` export, toolchains and cargo tools, `.rust-port/` run folder
- Workspace skeleton: root `Cargo.toml`, three crates, features, lints, `deny.toml`, rustfmt/clippy config
- AGENTS.md files (root layout, strands-rs, harness-rs), `docs/TESTING.md`, `docs/PARITY.md`
- CI workflows and `ci.yml` path filters (test-lint, MSRV, deny, doctests)
- Conformance fixture generator (tsx) and committed fixtures
- Run logs and `DIVERGENCES.md` files created
- Phase review gate and fixes

Details:
- Workspace: resolver 3, edition 2024, `rust-version = "1.96"`, workspace lints (`unsafe_code = "forbid"`, `missing_docs`, clippy `all`, and selective pedantic denies such as `unwrap_used`, `expect_used`, `panic`, `todo`, `unimplemented`, `dbg_macro`, `print_stdout` outside the printer and examples, `needless_pass_by_value`, `redundant_clone`, `await_holding_lock`, `await_holding_refcell_ref`, `await_holding_invalid_type`; choose a coherent set and document it in `strands-rs/AGENTS.md`), workspace dependencies with `preserve_order`, the `cedar-policy` pin, trimmed `aws-config` features. `rustfmt.toml` (for example `max_width = 110`, edition 2024). `clippy.toml` with the `allow-*-in-tests` settings. `deny.toml` covering licenses (an Apache-2.0-compatible allowlist), bans (duplicate detection, deny wildcards), advisories and sources (crates.io only). Crate manifests per section 8.1.
- Crates compile with placeholder `lib.rs` files that carry crate-level rustdoc.
- Repo-root edits (#76): the root `AGENTS.md` monorepo layout gains `strands-rs/` and `harness-rs/` lines and points to their `AGENTS.md`; `.gitignore` gains `target/`.
- `strands-rs/AGENTS.md` and `harness-rs/AGENTS.md`: purpose, layout, the gates, the code rules (section 7), the wire-compat rules (section 10's persisted layout and section 11's wire items), the testing rules, and how to regenerate fixtures. These files are public, so they contain no internal context. Add `CLAUDE.md` files containing `@AGENTS.md`, matching the repo convention.
- `strands-rs/docs/TESTING.md`, modelled on the donor's `strands-rs/docs/TESTING.md` (layout, fixtures, rules, running, live tests through env).
- `docs/PARITY.md`: one row per in-scope TS spec file (Appendix B), initially `0/N`.
- CI, mirroring the existing workflow structure (read `ci.yml`, `harness-py-pr-and-push.yml`, `harness-py-test-lint.yml`, `harness-ts-*.yml` and `typescript-security-audit.yml` at the pin):
  - `ci.yml` is the merge gate: `detect-changes` uses `dorny/paths-filter` with one output per area, each area job calls a reusable `workflow_call` workflow, and `ci-gate` uses `re-actors/alls-green` with an explicit `allowed-skips` list. A new area must be added to the outputs, filters, a job, `needs` and `allowed-skips`. Add `strands-rs` and `harness-rs` areas, with filters on `strands-rs/**/!(*.md)` and `harness-rs/**/!(*.md)`, their own workflows, `ci.yml`, root `Cargo.*`, `deny.toml`, plus `strands-ts/src/**` for the conformance job and `strands-rs/**` for harness-rs (it depends on the SDK).
  - `strands-rs-pr-and-push.yml` (calls test-lint and the security audit), `strands-rs-test-lint.yml` (fmt, clippy with all features and with no default features, tests both ways, doc with `-D warnings`, doctests, an MSRV job on 1.96.0, the conformance regeneration diff), `harness-rs-pr-and-push.yml`, `harness-rs-test-lint.yml`, and `rust-security-audit.yml` (cargo-deny).
  - Use `dtolnay/rust-toolchain` and `Swatinem/rust-cache`, SHA-pinned like the newer workflows, on the runner label the existing workflows use.
  - CI can't run where you are; validate YAML syntax (`python3 -c "import yaml,sys; yaml.safe_load(open(sys.argv[1]))" <file>` after `pip install pyyaml` if needed, or `npx --yes yaml valid < <file>`), install and run `actionlint` if you can, and mirror the existing patterns exactly. List the workflows as "not run" in the final report.
- An optional complexity adapter for `.github/scripts/pr-metrics` (Rust is not scored today; a `rust-code-analysis-cli` adapter would parse without compiling, which fits that workflow's "parse, never execute" rule). If you skip it, log that in pending.
- Conformance generator: `strands-rs/conformance/generate.ts` (run with tsx), importing the built strands-ts at the pin and writing JSON fixtures to `strands-rs/conformance/fixtures/`. Cover messages and every block, stream events (`ModelStreamEvent` and `AgentStreamEvent` `toJSON`), hook-event `toJSON`, snapshots, session manifests and immutable history, interrupt state, stash records, metric and span names, and the edge cases in section 11 items 24-26. Add an npm script or documented command to regenerate, and a CI step that regenerates and runs `git diff --exit-code`. The Rust tests that consume each fixture arrive with the phase that implements the types.
- Logs: `.rust-port/DECISIONS-pending.md` (header, format line, P-1 for `--no-verify`), `.rust-port/ts-bugs.md` (header and format line), and `strands-rs/docs/DIVERGENCES.md` and `harness-rs/docs/DIVERGENCES.md` with a table header.

### Phase 1: types, wire and errors

Tracker items:
- Messages, 14 content blocks, `JsonBlock`, media and citations
- Tool types, `StopReason`, `Usage`, `Metrics`, snapshot and interrupt types
- `strands::Error` and TS-mirrored error types with exact messages
- Constructors for non-exhaustive wire structs
- Fixture round-trips byte-identical
- Phase review gate and fixes

Details:
- `types`: `Message` with role, content and metadata; the 14 content blocks, with Reasoning's base64 `redactedContent`; `JsonBlock` in tool results; media types and citations; `ToolSpec`, `ToolUse`, `ToolResultBlock`, `ToolChoice`; `StopReason` (16 values plus `Other`) and the limit stop-reason set (crate-internal); `Usage`, `Metrics`; `SystemPrompt`/`SystemContentBlock` (snapshots carry them).
- Serde matches TS JSON: camelCase, `None` omitted, single-key block wrappers, text as a bare `{text}`, key order.
- Module placement mirrors TS file stems (for example `Usage`, `Metrics` and the `ModelStreamEvent` variants in `models/streaming.rs`; the interrupt types in `interrupt.rs`; `ToolSpec` and friends in `types/tools.rs`, because TS keeps them in `tools/types.ts`). Log the placement.
- Snapshot types (`Snapshot`, scope, `schemaVersion`) and interrupt types (`InterruptParams` as an option struct with `new(name)` and `From<&str>`/`From<String>`; `InterruptResponse`, `InterruptResponseContent` and `Interrupt` as wire structs).
- `error` per section 8.10, with exact TS messages (`src/errors.ts`, `src/sandbox/errors.ts`, the error classes in `src/mcp/client.ts`).
- Constructors on every `#[non_exhaustive]` wire struct.
- Round-trip every Phase 0 fixture byte-identically (parse, serialize, compare bytes with the TS-written JSON), plus tests for the section 11 wire items.
- TS specs: `types/__tests__/{messages, media, citations, json, agent (the AgentResult parts land in Phase 3), validation}.test.ts`, `__tests__/{errors, interrupt, mime}.test.ts`, and the type parts of `models/__tests__/streaming.test.ts`.

### Phase 2: model abstraction, streaming and retry

Tracker items:
- `ModelStreamEvent`, `ModelRequest`, `ModelState`, `BaseModelConfig`, `CacheConfig`
- `Model` trait with `stream_aggregated`, `summarize`, `count_tokens`, `estimate_utilization`, model info
- Retry strategy plugin, `DefaultModelRetryStrategy`, backoffs and jitter
- `strands::testing` mock model, hook recorder, test agent
- Phase review gate and fixes

Details:
1. `state_store.rs` (`StateStore`; port `src/state-store.ts` and `__tests__/state-store.test.ts`).
2. `cancel.rs` primitives (`CancelSignal`, `CancelHandle`, `CancelOptions`) with tests. The full cancel semantics are Phase 4.
3. `AgentMetadata` (`agent/agent-metadata.ts`), and `ModelInfo` with capabilities and limits (`models/model-info.ts`).
4. `models/model.rs`: the trait (section 8.4), `BaseModelConfig`, `ModelRequest`, `CountTokensOptions`, `SummarizeOptions`, `SummarizeResult`, `StreamAggregatedResult`, `AggregatedEvent`, `CacheConfig`, `CacheTtl`, resolved cache sections, `resolve_config_metadata`, `resolve_max_tokens`, `latest_user_index`. Port `models/model.ts` fully: `streamAggregated` block accumulation, the citations accumulator, tool-input JSON parse errors, the TS aggregation error texts, `MaxTokensError` carrying the partial message, redaction handling; the `countTokens` heuristic; the client-side `summarize` default (`models/summarize.ts`, exact prompt text); `estimateUtilization`.
5. `models/model_info.rs`: the embedded shared table (section 8.4), lookup and merge rules, the context-window default, warn-once (`logging/warn-once.ts`).
6. `models/defaults.rs`: `MODEL_DEFAULTS` per provider and the default-model-id warning text (`models/defaults.ts`).
7. `models/strict_schema.rs` and `models/native_output.rs` as crate-internal helpers (TS marks them `@internal`).
8. `retry/`: `ConstantBackoff`, `LinearBackoff`, `ExponentialBackoff` and the jitter kinds with an injectable RNG; the `ModelRetryStrategy` trait; `DefaultModelRetryStrategy` (6 attempts, 4 s base, 240 s max, only `ModelThrottledError` and `ModelNetworkError`). The hook wiring (sleep inside `AfterModelCallEvent`, set `retry`) needs Phase 3, so implement the decision logic and the injectable sleeper now, port the agent-free tests now, and wire it in Phase 3.
9. `strands::testing`: `MockModel`, ported from `__fixtures__/mock-message-model.ts` and the model test helpers (scripted turns, errors including throttles, delays through the injected sleeper, request recording). `HookRecorder` and `test_agent` come in Phase 3.

TS specs: `models/__tests__/{model (non-provider parts), model-info, model-info-lookup, defaults, strict-schema, native-output, server-side-summary-capability (trait parts), cache-portability (generic parts), streaming}.test.ts`, `retry/__tests__/*`, `__tests__/state-store.test.ts`, `logging/__tests__/warn-once.test.ts`. `model-proxy.ts` needs `InvocationContext`, so it goes in Phase 3. Donor: `models/model.rs`, `models/streaming.rs`, `models/defaults.rs`, `retry/*`, `state_store.rs`, `testing.rs`.

### Phase 3: agent loop, hooks and tools

Tracker items:
- `Agent`, `AgentConfig`, builder, lazy default model, `invoke`/`stream`/`AgentStream`, `AgentResult`
- Serializable stream event records (fixture-checked)
- Hook events, setters, `HookOrder`, `HookCleanup`, three registration forms
- `LocalAgent`, `ToolAgent`, `AppState`, `ModelState`, `InvocationState` handles
- `Tool`, `ToolContext`, `ToolOutcome`, `ToolRegistry`, `FunctionTool`, `ToolSource`, `tool_list!`, `ToolError`
- `#[tool]` and `#[tools]` macros with schema normalization, validation, trybuild tests
- Concurrent and sequential executors
- Loop semantics: deferred append, limits, `InvocationContext`, structured output, `end_turn`/`resume`
- Direct tool calls, `as_tool`, agents as tools, printer
- Phase review gate and fixes

Details (sections 8.5-8.9 give the shapes):
- `Agent`, `AgentConfig` (every TS key except `checkpointing`), the bon builder with a fallible `build()?`, and `Agent::new`. The lazy default Bedrock model (#45): `build()` stays sync and the model resolves on first use. Without the `bedrock` feature and with no model set, the first call fails with a clear `DefaultNotConfiguredError`-style message.
- `invoke`, `invoke_with`, `stream`, `stream_with`, `AgentStream<'_>` (named, `Unpin`), the `InvokeArgs` conversions, and `InvokeOptions` (structured output schema via `structured_output_schema::<T>()`, cancel signal, invocation state, invocation context, limits, and every other TS invoke option).
- `AgentResult` with `Display`, `structured_output::<T>()`, `stop_reason`, `interrupts`, `AgentMetrics`, `AgentTrace`.
- Stream items: the 16 owned records, checked against the fixtures.
- Hooks: the 17 events, getters, setters per 8.5, `HookOrder`, reverse order on `After*`, `HookCleanup`, `add_hook`, `add_async_hook::<E>`, `add_hook_impl`, interrupt collection, duplicate-name errors.
- Agent views and handles per 8.3 and 8.7.
- Tools per 8.8, including `ToolRegistry` and `FunctionTool` (`FunctionTool::builder(..)` with a typed handler: schemars schema plus jsonschema validation).
- `strands-macros`:
  - `#[tool]` on functions keeps the function and generates a PascalCase unit struct implementing `Tool`. The fn doc comment is the description and param doc comments are property descriptions. `#[tool(name = "..")]` overrides the name. `#[tool(context)]` marks the `&ToolContext` param explicitly (no name sniffing). The return type is `Result<T: Serialize, E: Into<ToolError>>` or a plain serializable value. Sync fns run on `spawn_blocking`.
  - `#[tools]` on impl blocks generates `X::<method>_tool(self: &Arc<Self>)` and `X::tools(self: &Arc<Self>) -> Vec<ToolSource>`.
  - Use `darling` for attribute parsing. Normalize schemas per 8.8. Report serde failures as tool validation errors.
  - Hygiene: generated code names dependencies only through a `#[doc(hidden)] pub mod __private` in `strands` that re-exports `serde`, `serde_json`, `schemars`, `jsonschema`, `async_trait` and whatever else it needs, so users don't need those crates. `strands/src/lib.rs` has `extern crate self as strands;` so the SDK's own vended tools can use `#[tool]`. Add a test crate (trybuild pass case or a workspace-excluded crate) that depends only on `strands` and `tokio`, because doctests can see dev-dependencies and hide missing paths. Compile it with `#![deny(warnings, missing_docs, clippy::pedantic, clippy::unwrap_used, clippy::expect_used, clippy::allow_attributes_without_reason)]` using `#[tool]`, `#[tools]`, `tool_list!` and hook registration, so generated code is clean under a strict downstream lint profile.
  - The macro strips `///` doc comments and `#[tool(..)]` attributes from parameters in the function it re-emits (rustc rejects doc comments on parameters after expansion), and `#[tools]` consumes the inner `#[tool]` attributes. The generated struct copies the function's visibility and doc comment so `missing_docs` passes. Prove parameter docs and `#[tool(context)]` with a trybuild pass case at the start of this item.
  - trybuild compile-fail tests for: a missing doc comment (decide and log whether that is an error or an empty description, following TS's behaviour for missing descriptions), a non-serializable return, two context params, `self` by value, generic fns, invalid names.
- Executors per 8.8.
- Loop semantics: deferred append, limits in the TS check order, `InvocationContext` (nested limits and usage), structured output through `strands_structured_output` with retries, `end_turn` and `resume`, and `model-proxy.ts`.
- Direct tool calls: `agent.tool("name").invoke(input).await` (TS `agent.tool.<name>.invoke()`), recorded into history as TS does (`agent/tool-caller.ts`).
- `as_tool` with `preserve_context` and `delegate`; agents accepted in `.tools(..)`.
- The stdout printer (#54): on by default, `.printer(false)` to turn it off, matching TS output (`agent/printer.ts`, `printer.test.ts`).
- Wire the retry strategy into `AfterModelCallEvent` and port the remaining retry tests.
- TS specs: `agent/__tests__/{agent, agent.hook, agent.concurrent (adapt it: concurrent invocation is a compile error here, so document that gap), agent.invocation-context, agent.invocation-state, agent.model-retry, agent-as-tool, agent-as-tool.invocation-context, agent-as-tool.invocation-state, agent-delegation, agent.continuation, invocation-context, printer, tool-caller}.test.ts`, `hooks/__tests__/{events, registry}.test.ts`, `tools/__tests__/{tool, tool-factory, structured-output-tool, zod-tool (map it to the macro and FunctionTool)}.test.ts`, the parts of `tools/executors/__tests__/executor.cancel-signal.test.ts` that don't need Phase 4, `registry/__tests__/tool-registry.test.ts`, `types/__tests__/agent.test.ts`, `__tests__/{queue, default-slot, index}.test.ts` where they apply, and `models/__tests__/model-proxy.test.ts`.

### Phase 4: interrupts, cancellation, lifecycle, plugins and middleware

Tracker items:
- Interrupts and resume with `pendingToolExecution`
- `CancelHandle`, `CancelOptions`, `CancelSignal`, TS cancel semantics
- Drop guard history repair, deferred cleanup, `shutdown()`, drop warning
- Snapshots (`take_snapshot`/`load_snapshot`)
- `Plugin` trait and built-in registration order
- Middleware stages and phases
- Background tasks
- Conversation managers (trait, Null, SlidingWindow, Summarizing)
- Phase review gate and fixes

Details:
- Interrupts: `ctx.interrupt()` returns the stored response on resume; TS-deterministic ids; `pendingToolExecution` resume with completed sibling results; interrupt state in snapshots; hook interrupts from `BeforeToolCallEvent` and `BeforeToolsEvent`; `InterruptEvent`.
- Cancellation per section 10 (checkpoints, synthetic cancelled results, the final assistant message, `after_current_tools`, the external `CancelSignal`).
- The drop guard (#63), and `shutdown()` with the drop warning (#64). Test dropping mid-model-stream, mid-tool and after an interrupt; history must be re-invokable after each, and the deferred `AfterInvocationEvent` must run on the next call.
- Snapshots: `take_snapshot`/`load_snapshot` with the presets and include/exclude rules (`agent/snapshot.ts`), in the TS `createdAt` format.
- `Plugin` trait (`name`, `init_agent` with a registrar that reaches `Arc<Self>`, `get_tools`), duplicate-name errors, and the fixed built-in registration order (section 10).
- Middleware: `add_middleware` registers per-stage handlers for `InvokeModelStage` and `ExecuteToolStage`, with Input, Wrap and Output phases. Input handlers transform the context (with TS copy-on-input semantics), Output handlers transform the result, and Wrap handlers receive the context and `next` and return a stream of stage events that ends in the stage result (the same shape as `Tool::stream`). Middleware contexts have `interrupt()`. Support custom stages if TS exports them (`custom-stages.test.ts`). Also implement `AgentStreamStage` (TS `middleware/stages.ts`, interrupt id `middleware:agentStream`): every invocation runs through it, and the middleware specs use it heavily. TS exports it from `middleware/index.ts` but not the root barrel; mirror that (export it from `strands::middleware` only) and port its test cases.
- Background tasks: the plugin, its config, the `strands_manage_background_task` tool, the `_background_execution` schema property, and the in-process engine and manager with the TS state transitions and an injectable clock.
- Conversation managers: the trait with proactive compression (threshold 0.7); `NullConversationManager`; `SlidingWindowConversationManager` (window 40, tool-pair and user-anchor trim rules, tool result truncation); `SummarizingConversationManager` (ratio 0.3 clamped to 0.1-0.8, preserve 10 recent messages, the aux model order); the token-usage middleware; pinning if it lives here at the pin; the interplay with stateful models.
- TS specs: `agent/__tests__/{agent.interrupt, agent.cancel, agent-as-tool.interrupt, snapshot, agent.stateful-model}.test.ts`, the rest of `tools/executors/__tests__/executor.cancel-signal.test.ts`, `plugins/__tests__/{plugin, registry}.test.ts`, `middleware/__tests__/*`, `background-tasks/**/__tests__/*`, and `conversation-manager/__tests__/{conversation-manager, null-conversation-manager, sliding-window-conversation-manager, summarizing-conversation-manager, context-compression, token-usage-middleware, pin, agentic-context (the conversation-manager parts; the rest in Phase 6)}.test.ts`.

### Phase 5: providers

Tracker items:
- `BedrockModel` (ConverseStream, caching, guardrails, throttling)
- `AnthropicModel` (reqwest SSE, transport retry, error mapping, thinking, caching, compaction, server tools)
- `OpenAIModel` (Responses, Chat, stateful, params, caching, error mapping, transport retry)
- Bedrock Mantle (region, routing, SigV4 bearer token)
- Live tests for Bedrock, Mantle, Anthropic, OpenAI
- Phase review gate and fixes

Details:
- `bedrock` feature, `BedrockModel`: ConverseStream via `aws-sdk-bedrockruntime`; format mapping for every block, and the reverse parse; `BedrockCacheConfig`; guardrails and redaction; throttling and overflow mapping; the TS default model id and region resolution; a `from_client` seam.
  - Write your own `serde_json::Value` to and from `aws_smithy_types::Document` converter (the serde bridge is gated on `--cfg aws_sdk_unstable`). Map non-negative integers to `PosInt`, negative integers to `NegInt` and everything else to `Float`; test `u64::MAX`, `i64::MIN`, `0`, `-0.0` and floats in both directions.
  - `Document::Object` is a `HashMap`, so key order can't survive into the request. Keep order up to the SDK boundary, build each tool-spec and history `toolUse` `Document` once per agent and reuse clones (verify a clone keeps iteration order) so cache prefixes stay stable within a process, log the remaining difference in `DIVERGENCES.md`, and include tools in the live cache-hit test.
  - Adapt the ConverseStream `EventReceiver` with `stream::unfold`. Every `recv()` error becomes a mapped `Err` item, never a silent end of stream. Classify throttling, service-unavailable and model-stream errors through the typed `ConverseStreamOutputError`/`SdkError` variants, never by matching `Debug` text. A stream that ends without `messageStop` is an error.
- `anthropic` feature, `AnthropicModel`: the Messages API over `reqwest`, with SSE via `eventsource-stream`; the vendor-SDK transport retry (#68: 2 retries on connection errors, 408, 409, 429 and 5xx; honour `x-should-retry` and `retry-after`; 0.5-8 s exponential backoff; a 10-minute timeout); error mapping (429 to throttled, the six context-overflow patterns, transport to network); thinking and adaptive thinking; caching; server-side `summarize` through on-demand compaction; server tools, including the native `web_search` hook the harness needs; the API key from config or `ANTHROPIC_API_KEY`; a configurable base URL. Replicate the official TS SDKs' environment defaults for both HTTP providers: `ANTHROPIC_BASE_URL`, `OPENAI_BASE_URL`, and any other env defaults their client constructors read (check `node_modules/@anthropic-ai/sdk` and `node_modules/openai` after `npm ci`), read through the env seam. The harness depends on this (`harness-ts/src/models.ts`: with a custom base URL, `web_fetch` model resolution reuses the main model and warns once with the TS text). Redact the key in `Debug`.
- `openai` feature, `OpenAIModel`: Responses (default) and Chat Completions; `stateful` with `previous_response_id` in `ModelState`; `params` passthrough with warnings for provider-managed keys; `CacheConfig`; error mapping (429, `rate_limit_exceeded`, message patterns, the eight overflow patterns); the same transport retry; native `web_search` support for the harness. In Chat Completions, gather tool-call deltas by `index` in an ordered map and emit them in index order; test with three parallel tool calls whose deltas interleave.
- Mantle (`openai` plus `bedrock`): `bedrock_mantle_config` with region resolution and validation, base-path routing including the cross-base-path reroute in `update_config`, and the SigV4-presigned bearer token (section 10).
- Live tests for each provider and for Mantle (9.6): text, streaming, a tool round trip, structured output, and a cache hit where the provider reports one.
- TS specs: `models/__tests__/{bedrock, anthropic, cache-portability (provider parts), server-side-summary-capability (provider parts)}.test.ts`, `models/openai/__tests__/{responses, chat, errors, mantle, responses-cache}.test.ts`, with the integ files under `strands-ts/test/integ/models/` as the live checklist.

### Phase 6: storage, sessions, context and memory

Tracker items:
- `Storage`, `InMemoryStorage`, `LocalFileStorage`, namespaces, `KeywordSearchStrategy`
- `SessionManager` with TS layout and cross-SDK fixtures
- Experimental `ContextManager` (presets, offload strategies, stash, `retrieve_context`, pinning, agentic tools)
- `ContextInjector` and injection with `InjectedBlock`
- `MemoryManager`, `MemoryStore`, `FileMemoryStore`, `ModelExtractor`
- `AgentSkills` and `Skill`
- Phase review gate and fixes

Details:
- The `Storage` trait (bytes, `list(prefix)`, namespaces, optional search, key validation), `InMemoryStorage`, `LocalFileStorage` (atomic temp-and-rename writes, sandbox-aware where TS is), namespaced storage, and `KeywordSearchStrategy`.
- `SessionManager`, a concrete type as in TS (the seam is `Storage`): config and `save_latest_on`; restore on `InitializedEvent`; the TS key layout with the manifest and uuidv7 immutable history; redaction saves; batched deletes; the id rules (`^[a-z0-9_-]+$`); the snapshot storage adapter. Cross-SDK fixture tests: Rust loads TS-written sessions, and Rust output matches the fixtures.
- `strands::experimental::ContextManager` (#61): the `auto` and `agentic` presets; the `Offload` strategies (truncate, summarize, emergency truncate) and named presets; the stash at `context/<sessionId>/...`, inline or external, with `storageType` TS class names; the `retrieve_context` tool; pinning; the `summarize_context`, `truncate_context` and `pin_context` tools; the token middleware; `Model::summarize` integration. The harness's `HARNESS_CONTRACT` promises that offloaded results can be retrieved, so this has to work for the harness default.
- `ContextInjector` and injection: the `userTurn` trigger, `InjectedBlock`, `dynamic_trailing_blocks`, failing open on injector errors, XML escaping.
- Memory: `MemoryManager` (the `search_memory` and `add_memory` tools, injection, defaults 3 and 5, extraction triggers, `ModelExtractor` on the aux model order, `flush`), the `MemoryStore` trait, a read-only view, and `FileMemoryStore` (the `memory/<name>` namespace, `.md` files, a keyword fallback). Extraction runs spawned, not inside lifecycle hooks; `shutdown()` flushes.
- `AgentSkills` and `Skill` (the `skills` tool, `skill_name`, discovery through the sandbox or storage as TS does).
- TS specs: `storage/__tests__/{in-memory-storage, namespaced-storage}.test.ts`, `storage/search/__tests__/keyword.test.ts`, `session/__tests__/{session-manager, snapshot-storage-adapter, validation}.test.ts`, `agent/__tests__/{agent.storage, agent.context-manager}.test.ts`, `context-manager/**/__tests__/*`, the rest of `conversation-manager/__tests__/agentic-context.test.ts`, `injection/__tests__/message-injection.test.ts`, `vended-plugins/context-injector/__tests__/plugin.test.ts`, and `memory/**/__tests__/*` (the tracing parts after Phase 10). The S3, s3-vector and embeddings specs are out of scope; note them in PARITY.

### Phase 7: interventions and routing

Tracker items:
- `InterventionHandler` and registry
- `HumanInTheLoop` with classifiers
- `CedarAuthorization`
- `ModelRouter` with `FallbackStrategy` and `ClassifierStrategy`
- Phase review gate and fixes

Details:
- `InterventionHandler` and its registry: `on_error` values `throw`, `proceed` and `deny`; actions `proceed`, `deny`, `guide`, `confirm` and `transform` (gate-specific action types, so illegal actions can't be expressed; `transform` as a deferred closure on the event); the `HookOrder` intervention input and output orders; `guide` writes directly to messages, as in TS; TS's sequential short-circuit precedence.
- `HumanInTheLoop`: an `ask` callback, or interrupt and resume by default; trusted tools in app state; classifiers that are boolean, LLM (`createLlmRiskClassifier`, structured output, the model order) or a function. No decision-model classifier.
- `CedarAuthorization` on `cedar-policy`: policies from text or a file, entities, the schema (port `schema-generator.ts`), the rate-limit state key `cedar-authorization`, and proceed and deny with the TS texts.
- The `ModelRouter` plugin: `FallbackStrategy` and `ClassifierStrategy` (with `routingPolicy`, not the deprecated `systemPrompt`), rejection of stateful models and duplicates, `max_switches`, the attempt-count reset; `aux_model` rejects routers.
- TS specs: `interventions/__tests__/{handler, registry}.test.ts`, `vended-interventions/hitl/__tests__/hitl.test.ts`, `vended-interventions/cedar/__tests__/cedar.test.node.ts` (plus the no-network parts of `test/integ/cedar.test.node.ts`), `models/routing/__tests__/{router, fallback-strategy, classifier-strategy}.test.ts`, and `agent/__tests__/agent.model-routing.test.ts`.

### Phase 8: sandbox and vended tools

Tracker items:
- `Sandbox` trait, protocol, errors, default local sandbox (POSIX and PowerShell)
- `shell`, `read`, `write`, `edit`, `bash`
- `python_repl`, `notebook`, `http_request`, `sleep`, `stop`, `handoff_to_user`
- `web_fetch` and `subagent`
- Phase review gate and fixes

Details:
- The `Sandbox` trait: streaming exec and code exec, file operations, `ExecuteOptions`, the `StreamChunk`/`ExecutionResult` protocol, `get_tools`, and the sandbox errors (including `SandboxIsADirectoryError` and `SandboxPermissionError`).
- The default local sandbox: a POSIX shell with an env prefix, and a Windows PowerShell `-EncodedCommand` wrapper. `sandbox: false` produces `DefaultNotConfiguredError` when a tool needs a sandbox. Keep the TS naming of the local sandbox type; don't rename it to something more reassuring than TS uses.
- Vended tools, all routed through the sandbox and matching TS names, schemas and text: `shell` (a configurable default timeout of 120 s, exit code 124 on timeout), `read`, `write`, `edit`, `bash` (a persistent session), `python_repl`, `notebook`, `http_request` (stream-capped body), `sleep` (injectable sleeper), `stop`, `handoff_to_user` (an interrupt), `web_fetch` (on `htmd`, with the aux model order for summarization, caps and cache), `subagent` (the SDK's vended one; the harness one is Phase 12), and `mcp_router` after Phase 9. Also the `vended-tools/paths.ts` path rules.
- TS specs: the in-scope `vended-tools/**/__tests__/*` (`paths`, `handoff-to-user`, `http-request`, `notebook`, `sleep`, `stop`, `subagent`, `web-fetch`, `extract`), the node-only `vended-tools/{bash, edit, read, write, shell, python-repl, mcp-router}/__tests__/*.test.node.ts`, and `sandbox/__tests__/{default, errors, not-a-sandbox-local-environment, posix-shell, read-file-errors, stream-process}.test.node.ts`. Skip `graph`, `swarm`, `a2a-client` and `fileEditor`.

### Phase 9: MCP

Tracker items:
- `McpClient` stdio and streamable HTTP
- Custom legacy SSE transport
- Filters, prefixes, auth, elicitation, prompts, resources, list-changed, progress, logging, trace context
- `load_servers` config loading
- `mcp_router` tool
- Phase review gate and fixes

Details:
- The `mcp` feature and `McpClient` on `rmcp` 3.x: stdio (env merged over the default environment), streamable HTTP, and a custom legacy SSE transport on `rmcp`'s `Transport` trait. Match TS's legacy-fallback behaviour (`test/integ/mcp/mcp-legacy-fallback.test.node.ts`).
- Tool filters (exact, regex, callback), `<prefix>_<name>`, `continue_on_error`, and the `failed` state.
- Auth: client credentials, an auth provider, headers.
- The elicitation callback; prompts and resources; list-changed notifications (TS `onToolsChanged`); progress; logging; `list_max_pages`; `requestTimeout` (not the deprecated `ttl`).
- W3C trace context in `_meta`, unless disabled.
- Client identity: name `strands-agents-rs-sdk`, version = the crate version (TS sends `strands-agents-ts-sdk`). Log it in pending.
- `McpClient::load_servers`: a path or a map; flat or `mcpServers`-wrapped; `${VAR}`/`${env:VAR}` expansion; `disabled` and `cwd`; the TS error rules; `prefix_with_server_name`.
- `disconnect().await` and drop cleanup. No async mutex across RPCs; concurrent calls to one server work.
- `mcp_router`, with `tool_filters` enforced on `call_tool`.
- Test servers: use the TS test MCP servers where practical (`harness-ts/test/tools/echo-mcp-server.mjs`, `strands-ts/test/integ/__fixtures__/test-mcp-server.ts`) via node, in feature-gated tests that skip without node, or write a small rmcp test server in Rust for the unit suite.
- TS specs: `mcp/__tests__/{client, client-annotations, client-progress}.test.ts`, `vended-tools/mcp-router/__tests__/mcp-router.test.ts`, with the non-task integ MCP files as a checklist. Skip `task-client` and `task-transport`.

### Phase 10: telemetry

Tracker items:
- Spans and attributes with semconv modes
- Meter counters and histograms
- `otel` feature exporters and env configuration
- Phase review gate and fixes

Details:
- `tracing` spans with the TS names and attributes and the `gen_ai.*` semantic-convention modes (`OTEL_SEMCONV_STABILITY_OPT_IN`), legacy usage keys duplicated, Langfuse detection, resource attributes, the service name default `strands-agents`, the agent `trace_attributes` and `baggage_attributes`, and attribute JSON serialization as TS does it (`telemetry/json`).
- The meter's counters and histograms with the TS names and units.
- The `otel` feature: `setup_tracer` and `setup_meter` with OTLP HTTP and console exporters, configured from env, idempotent.
- Check the names against the conformance fixture.
- `trace_attributes` keys are dynamic and typed (string, number, bool, arrays), and TS merges them into every span (agent, cycle, model, tool; `telemetry/tracer.ts`). `tracing` field names are static, so apply them through `tracing_opentelemetry::OpenTelemetrySpanExt::set_attribute` under `otel` (and record them in `AgentTrace`), never as one serialized JSON field. Test with the recording exporter.
- Define the `gen_ai.*` attribute keys as crate constants rather than using the semantic-conventions crate's constants, some of which are `#[deprecated]` and fail `-D warnings`.
- Test spans with a `tracing-opentelemetry` layer over an `opentelemetry_sdk` in-memory exporter. Scoped `set_default` subscribers are thread-local, so run those tests on a `current_thread` runtime. Include a test that `execute_tool` spans from spawned tools parent under the cycle span.
- TS specs: `telemetry/__tests__/{config, json, local-trace, meter}.test.ts`, `memory/__tests__/memory-tracing.test.ts`, and `logging/__tests__/logger.test.ts` where it maps to the `tracing` output format.

### Phase 11: SDK checkpoint

Tracker items:
- Full gates, MSRV, deny, doctests, all live tests green
- Parity sweep complete in `docs/PARITY.md`
- Crate docs, README and examples
- Whole-SDK review gate and fixes
- Local tag `strands-rs-v0.1.0-preview.1`

Details:
- Full gates, MSRV, `cargo deny check`, doctests, and each provider feature alone.
- All live tests green, or failures recorded with exact errors, in `strands-rs/docs/reviews/live.md`.
- Parity sweep: every in-scope TS spec file has a row, and every gap has a reason or is fixed.
- Crate docs: crate-level rustdoc with a module and feature overview, and the DX examples in section 13 as doctests; `strands-rs/README.md`; examples in `strands-rs/strands/examples/`: basic, tools, streaming, hooks, structured output, MCP, sessions, and one per provider (feature-gated with `required-features`).
- Whole-SDK review gate: all three reviewers across the full crate, then a fix loop until clean.
- Set the crate versions to `0.1.0-preview.1` and create the annotated tag `strands-rs-v0.1.0-preview.1` (a tag may not be collected with the branch, so also record the tagged commit in `STATE.md` and the report).
- Start the harness only after this checkpoint passes (#76).

### Phase 12: harness

Tracker items:
- `create_harness`, `HarnessAgentOptions`, defaults, sessions layout (#70)
- Model strings, effort, caching, provider tables
- `HARNESS_CONTRACT` and `build_system_prompt`
- Harness file tools, `shell`, `web_fetch`, `web_search` (native and Exa)
- `programmatic_tool_caller` on Monty
- `subagent` with presets and axes
- `Todos` and `EnvironmentContext` plugins
- Interventions grammar and `mcp_servers`
- Portable `HarnessAgentConfig` parsing and module-reference errors
- Telemetry setup from env
- Live end-to-end per provider and cross-SDK session resume
- Phase review gate and fixes

Details (harness-ts `src/` and `test/` at the pin are the spec):
- `create_harness(options).await -> Result<Agent>`, returning a plain SDK `Agent`. `HarnessAgentOptions` has every TS field plus `AgentConfig` passthrough, in struct and builder form. Decide whether the options builder's `build()` is fallible, consistently with the SDK, and log it. Use one enum per polymorphic option (`On`/`Off`/`Config`/`Instance`), with an explicit `Auto` where TS distinguishes unset from explicit.
- The `AgentConfig` passthrough mirrors TS `HarnessAgentOptions extends Omit<AgentConfig, 'model' | 'tools' | 'plugins' | 'interventions' | 'backgroundTasks' | 'contextManager'>` (`harness-ts/src/agent.ts`): those six are harness fields, so setting them in the passthrough returns a typed `HarnessError` (log the choice). Harness-resolved `session_manager`, `storage` and `memory_manager` override passthrough values, following TS's spread order. With an explicit sandbox, remove its vended tools after initialize as TS does. Test precedence per field.
- Defaults (#70): model `bedrock/global.anthropic.claude-opus-5`; effort and caching `auto`; `ContextManager` `auto`; the 8 built-in tools (`shell`, `read`, `write`, `edit`, `web_fetch`, `web_search`, `programmatic_tool_caller`, `subagent`) and 2 plugins (`Todos`, `EnvironmentContext`); sessions under `./.agent/sessions` through `LocalFileStorage::new("./.agent").namespace("sessions")`, with an 8-hex id and `save_latest_on: message`; the stash at `<dir>/<id>`; skills at `./.agent/skills`; memory at `./.agent/memory` with `ModelExtractor` on a small per-provider model; background tasks on, with `subagent` always run in the background; subagent max depth 2; telemetry from `OTEL_TRACES_EXPORTER`; `web_search` silently disabled on Bedrock.
- Models: the model-string grammar for `bedrock/`, `bedrock-mantle/`, `anthropic/`, `openai/`, with bare ids meaning Bedrock; the error texts in section 10 for disabled features, unsupported providers and unknown providers (`harness-ts/src/models.ts`); effort tables per provider and family (adaptive versus budget thinking, max-tokens tables); caching rules; `web_fetch` model selection (`resolveWebFetchModel`, `supportsMedia`); `supports_thinking` and `supports_web_search`.
- Prompt: `HARNESS_CONTRACT` byte-identical, `build_system_prompt`, and `instructions` versus `system_prompt`. Add a parity test that reads the TS prompt constants from `/tmp/ts-spec` when present.
- Built-in tools:
  - harness `read`, `write`, `edit` with the TS text and limits (`cat -n` numbering, media placeholders, error strings), through the sandbox;
  - `shell`;
  - `web_fetch`: curl in the sandbox or direct, allowlist, caps, the 15-minute URL cache, user agent `strands-harness/1.0`, the summarizer;
  - `web_search`: native for anthropic, openai and mantle; Exa over the SDK's `McpClient` with the TS privacy warning; silently off on Bedrock; explicitly enabling it on an unsupported model raises the TS error;
  - `programmatic_tool_caller` on `monty`: kwargs-only async functions, name aliasing, the TS limits, and inner calls through `ToolAgent::call_tool` so interventions gate them. Enforce the limits with monty's own resource limits so they stop execution hard (check which it offers; an elapsed-time check around the run is not a timeout), run interpreter steps on `spawn_blocking`, and test that an infinite loop stops at the limit. When `call_tool` reports a pending approval, raise a catchable guest `RuntimeError` with the exact TS text `Tool '<name>' needs approval (<reason>), which programmatic_tool_caller cannot prompt for; call it as a normal tool call instead.`; on cancel, `programmatic_tool_caller was cancelled; no further tool calls are made.` (`harness-ts/src/tools/programmatic-tool-caller.ts`);
  - `subagent`: the presets and axes (`AgentSpec`, `Fixed`, `Inherit`, `Open`, `Choice`, `ChoiceOption`, `Preset`), the context modes `none`, `all` and `no_tools` with verbatim preambles, the depth guard, child interrupts resumed through public results, the inheritance rules, and section 11 items 43-48;
  - name-collision rules with `-` and `_` treated as equal; built-in tool resolution (the list form pins, the mapping form edits the defaults, `"*"` must be a bool, TS errors for unknown names and keys).
- Plugins: `Todos` (`strands:todos`, the `todo_write` tool storing items under the TS app-state key, `activeForm` in camelCase, the every-turn `<system-reminder>` injection with TS strings) and `EnvironmentContext` (the TS discovery rules and caps, the `AGENTS.md` cap with char-boundary-safe truncation, nearby `AGENTS.md`/`README.md` links with the TS depth and skip rules, all through the sandbox, native date and platform, PowerShell shell detection on Windows).
- Interventions grammar: `off`, `ask`, `smart`, `*.cedar`, free text, handler instances, arrays, duplicate names (TS errors). `ask` without a callback interrupts on the first gated call, so `invoke` returns an interrupt.
- `mcp_servers`: a path or an inline map, `continue_on_error`, prefixes. Connected clients are reused for subagents.
- Portable config: `HarnessAgentConfig` (24 fields at the pin: `name`, `description`, `instructions`, `model`, `modelModule`, `effort`, `tools`, `subagents`, `mcpServers`, `builtinTools`, `caching`, `contextManager`, `session`, `skills`, `memory`, `memoryStores`, `plugins`, `builtinPlugins`, `interventions`, `interventionModules`, `sandbox`, `agentConfigModules`, `dependencies`, `agentConfig`; the `*Module`/`*Modules` fields are module references), `DEFAULT_HARNESS_AGENT_CONFIG`, normalization with unknown-key warnings and JSON-path errors (TS texts), and `harness_agent_options_from_config(config, base_dir)` with path and env expansion. Module references anywhere, including the nested `builtinTools.web_fetch.model`, produce the unsupported error from section 10 (`language` is optional and is `typescript` or `python`).
- Telemetry setup from `OTEL_TRACES_EXPORTER`, idempotent.
- Port the harness-ts specs (`harness-ts/test/*.test.ts`, `plugins/*`, `tools/*`), with the integration folder as the live checklist.
- Live end-to-end per provider (Bedrock, Mantle, Anthropic, OpenAI), each covering a tool call, an MCP tool (`harness-ts/test/tools/echo-mcp-server.mjs` via node), an interrupt and resume (`interventions: "ask"`), and a session resume. Run them in a temp directory, never inside the repo.
- Cross-SDK check: a session written by TS `createHarness` (harness-ts at the pin run via tsx, against a mock or real model, in a temp dir) is resumed by Rust, and the reverse, on the same `./.agent` directory. Record the commands and results in `harness-rs/docs/reviews/live.md`.

### Phase 13: final checkpoint and handoff

Tracker items:
- Full workspace gates and `cargo package --list` dry checks
- `IMPLEMENTATION-REPORT.md` with pending decisions, divergences and TS bugs
- `FOLLOW-UPS.md`, `STATE.md` and tracker finalized; working tree clean

Details:
- Full gates across the workspace, MSRV, `cargo deny check`, doctests, live tests, and the harness review gate closed.
- `cargo package --list -p <crate> --allow-dirty` for each crate, as a dry check only of what would ship. Never publish.
- `.rust-port/IMPLEMENTATION-REPORT.md`, in plain prose with sentence-case headings, facts only:
  - what was built (crates, modules, key public types, features);
  - test counts (from the gate script) and parity coverage (from `PARITY.md`: ported/total per area, gaps with reasons);
  - live results per provider, for the SDK and the harness, including any that couldn't run and why;
  - a summary of `DECISIONS-pending.md` for Arron's review, listing first the entries that change behaviour or public API the most, with why (no priority labels);
  - summaries of `DIVERGENCES.md` and `ts-bugs.md`;
  - known gaps, and anything skipped because it conflicted with a binding decision;
  - the branch, the first and last commit, the commit count, and the tagged commit.
- `.rust-port/FOLLOW-UPS.md` as `- [ ]` lines: the site-docs PR (#50), the parity automation workstream (#16), the later v0.x items from section 5.2, the CI items you couldn't validate locally, and anything you deferred.
- Tick the final tracker items and phase headings, update `STATE.md` to "complete", and leave the working tree clean with everything committed.

---

## 13. Developer experience targets (must compile as doctests or examples)

```rust
use futures::StreamExt;
use strands::{tool, Agent, AgentStreamEvent};

/// Get the current weather for a city.
#[tool]
async fn get_weather(
    /// City name, for example "Seattle".
    city: String,
) -> Result<String, strands::ToolError> {
    Ok(format!("Sunny in {city}"))
}

#[tokio::main]
async fn main() -> Result<(), strands::Error> {
    let mut agent = Agent::builder()            // default model: Bedrock
        .system_prompt("You are a concise assistant.")
        .tools([GetWeather])
        .build()?;

    let result = agent.invoke("What's the weather in Seattle?").await?;
    println!("{result}");

    {
        let mut stream = agent.stream("And in Lisbon?");
        while let Some(event) = stream.next().await {
            if let AgentStreamEvent::ModelStreamUpdate(update) = event? {
                if let Some(text) = update.text_delta() {
                    print!("{text}");
                }
            }
        }
    }
    agent.shutdown().await
}
```

The stream must be scoped (or dropped) before `shutdown()`; that is why `AgentStream` is a named `Unpin` type.

```rust
use strands::{Agent, AgentConfig, models::AnthropicModel};

let agent = Agent::new(AgentConfig {
    model: Some(AnthropicModel::builder().model_id("claude-sonnet-5-5").build()?.into()),
    system_prompt: Some("You are a concise assistant.".into()),
    ..Default::default()
})?;
```

```rust
use std::sync::Arc;
use strands::{tools, ToolContext};

struct Inventory { db: Pool }

#[tools]
impl Inventory {
    /// Look up stock for a product.
    #[tool]
    async fn check_stock(&self, #[tool(context)] ctx: &ToolContext, sku: String) -> Result<u32, strands::ToolError> {
        let _ = ctx.agent().app_state().get::<String>("region");
        Ok(self.db.stock(&sku).await?)
    }
}

let inventory = Arc::new(Inventory { db });
let agent = Agent::builder().tools(inventory.tools()).build()?;
```

```rust
use strands::hooks::{BeforeToolCallEvent, HookOrder};
use strands::{CancelOptions, InvokeOptions};

agent.add_hook(|event: &mut BeforeToolCallEvent| {
    if event.tool_use().name() == "shell" {
        event.set_cancel("shell is disabled");
    }
});

agent.add_async_hook::<BeforeToolCallEvent>(|event| Box::pin(async move {
    audit_log(event.tool_use()).await;
}));

#[derive(serde::Deserialize, schemars::JsonSchema)]
struct Summary { title: String, points: Vec<String> }

let summary: Summary = agent
    .invoke_with("Summarize this thread", InvokeOptions::default().structured_output_schema::<Summary>())
    .await?
    .structured_output()?;

let cancel = agent.cancel_handle();
tokio::spawn(async move {
    tokio::time::sleep(std::time::Duration::from_secs(30)).await;
    cancel.cancel(CancelOptions::default());
});
```

```rust
use strands::{InterruptResponse, StopReason};
use strands_harness::{create_harness, Effort, HarnessAgentOptions};

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
    let answers: Vec<InterruptResponse> = result.interrupts().iter().map(ask_user).collect();
    result = agent.invoke(answers).await?;
}
agent.shutdown().await?;
```

TS-to-Rust mapping:

| TS | Rust |
|---|---|
| `new Agent({...})` | `Agent::builder()...build()?` or `Agent::new(AgentConfig {..})?` |
| `agent.invoke(args, options?)` | `agent.invoke(args)` / `agent.invoke_with(args, options)` |
| `async *stream()` | `fn stream(&mut self, ..) -> AgentStream<'_>` (items: `Result<AgentStreamEvent>`) |
| `agent.cancel(options?)` | `agent.cancel_handle().cancel(options)` or `InvokeOptions::cancel_signal` |
| `event.agent` (`LocalAgent`) | `event.agent()` / `event.agent_mut()` on sequential events, `ToolAgent` on per-tool events |
| `addHook(Event, cb, { order })` | `add_hook` / `add_async_hook::<E>` / `add_hook_impl`, with an order option, returning `HookCleanup` |
| `tool({ name, inputSchema, callback })` | `#[tool]` on an `async fn`, `#[tools]` on an `impl`, or `FunctionTool::builder(..)` |
| `Tool.stream()` async generator | `Tool::stream()` returning a `Stream` that ends in `ToolOutcome` |
| `context.interrupt(params)` | `ctx.interrupt(params)?`, which returns the response on resume |
| `agent.tool.<name>.invoke()` | `agent.tool("name").invoke(input).await` |
| `AbortSignal` | `strands::CancelSignal` |
| Zod schemas | `serde` + `schemars` types |
| `Record<string, JSONValue>` state | `StateStore` (a JSON map with typed `get::<T>` / `set`) |
| `Record<string, unknown>` invocation state | `InvocationState` (a typed extensions map) |
| Optional peer dependencies | cargo features |
| `@strands-agents/sdk/experimental` | `strands::experimental` |
| `await using` / `Symbol.asyncDispose` | `shutdown().await` |

---

## Appendix A: alternatives already rejected (don't revisit)

- Adopting an existing port and refactoring it in place (its TS-shaped API would touch nearly every file; reuse its subsystems instead).
- `Agent` as a cloneable `Arc<Inner>` handle with `&self` methods (#22): every field behind a lock, concurrent invocation caught only at runtime, and mid-batch history mutation from concurrent tools with no defined semantics.
- A separate repo, a shared Rust or WASM core under the other SDKs (an earlier WASM experiment was removed), or endorsing a community crate.
- `#[tool]` generating a constructor function for free functions.
- `get_config() -> serde_json::Value`, or a config type parameter on `Model` (rules out `dyn Model`).
- `AsyncFn(&mut E)` hook callbacks (the future isn't provably `Send` on stable), closures returning `async move` blocks (can't borrow the event), and inferring an async hook's event type from the closure argument (the future's lifetime can't be inferred; hence the turbofish).
- A JSON-only `InvocationState` (can't carry live values).
- `#[non_exhaustive]` on config structs (E0639).
- A blanket `From<T: Tool> for Arc<dyn Tool>` (E0210).
- Async work in `Drop`, or spawning cleanup from `Drop` (the stream borrows the agent).

## Appendix B: in-scope TS spec files at the pin

Paths are under `strands-ts/src/`. Assign each file to a `PARITY.md` row. TS has both `*.test.ts` and node-only `*.test.node.ts` specs; both count. Out-of-scope specs (`a2a/`, `multiagent/`, `decisions/`, `experimental/checkpoint`, `agent.checkpoint`, `models/google`, `models/vercel`, `classifier-strategy.decision-model`, `mcp/task-*`, `session/s3-storage`, `storage/s3-storage`, `storage/embeddings/bedrock`, `storage/search/s3-vector`, `vended-interventions/steering/*`, `vended-plugins/context-offloader/*`, `vended-plugins/goal/*`, `vended-tools/{graph, swarm, a2a-client}`) get a row marked out of scope with the reason. Re-list the directory at the pin to catch anything this list misses.

- Root: `__tests__/{default-slot, errors, index, interrupt, mime, queue, state-store}.test.ts`
- `agent/__tests__/`: `agent-as-tool.interrupt`, `agent-as-tool.invocation-context`, `agent-as-tool.invocation-state`, `agent-as-tool`, `agent-delegation`, `agent.cancel`, `agent.concurrent`, `agent.context-manager`, `agent.continuation`, `agent.hook`, `agent.interrupt`, `agent.invocation-context`, `agent.invocation-state`, `agent.model-retry`, `agent.model-routing`, `agent.stateful-model`, `agent.storage`, `agent`, `invocation-context`, `printer`, `snapshot`, `tool-caller`
- `background-tasks/__tests__/background-tasks`, `background-tasks/in-process/__tests__/{engine-execution, engine-transitions, manager}`
- `context-manager/__tests__/{context-manager, presets, retrieval-tool, stash-integration, stash}`, `context-manager/strategies/__tests__/{offload-strategy, summarize-strategy}`
- `conversation-manager/__tests__/{agentic-context, context-compression, conversation-manager, null-conversation-manager, pin, sliding-window-conversation-manager, summarizing-conversation-manager, token-usage-middleware}`
- `hooks/__tests__/{events, registry}`
- `injection/__tests__/message-injection`
- `interventions/__tests__/{handler, registry}`
- `logging/__tests__/{logger, warn-once}`
- `mcp/__tests__/{client-annotations, client-progress, client}`
- `memory/__tests__/{memory-manager, memory-tracing}`, `memory/extraction/__tests__/{extraction, model-extractor, resolve-extraction-config}`
- `middleware/__tests__/{agent-middleware, copy-on-input, custom-stages, middleware-interrupts, registry}`
- `models/__tests__/{anthropic, bedrock, cache-portability, defaults, model-info-lookup, model-info, model-proxy, model, native-output, server-side-summary-capability, streaming, strict-schema}`, `models/openai/__tests__/{chat, errors, mantle, responses-cache, responses}`, `models/routing/__tests__/{classifier-strategy, fallback-strategy, router}`
- `plugins/__tests__/{plugin, registry}`
- `registry/__tests__/tool-registry`
- `retry/__tests__/{backoff-strategy, default-model-retry-strategy}`
- `session/__tests__/{session-manager, snapshot-storage-adapter, validation}`
- `storage/__tests__/{in-memory-storage, namespaced-storage}`, `storage/search/__tests__/keyword`
- `telemetry/__tests__/{config, json, local-trace, meter}`
- `tools/__tests__/{structured-output-tool, tool-factory, tool, zod-tool}`, `tools/executors/__tests__/executor.cancel-signal`
- `types/__tests__/{agent, citations, json, media, messages, validation}`
- `vended-interventions/hitl/__tests__/hitl`
- `vended-plugins/context-injector/__tests__/plugin`
- `vended-tools/__tests__/paths`, `vended-tools/{handoff-to-user, http-request, mcp-router, notebook, sleep, stop, subagent}/__tests__/*`, `vended-tools/web-fetch/__tests__/{extract, web-fetch}`

Node-only specs (`*.test.node.ts`), all in scope:
- `agent/__tests__/agent.tracer`, `mcp/__tests__/config`, `models/__tests__/model-info`
- `sandbox/__tests__/{default, errors, not-a-sandbox-local-environment, posix-shell, read-file-errors, stream-process}`
- `storage/__tests__/local-file-storage`
- `telemetry/__tests__/{config, tracer}`
- `vended-interventions/cedar/__tests__/cedar`
- `vended-memory-stores/file-memory-store/__tests__/file-memory-store`
- `vended-plugins/skills/__tests__/{agent-skills, skill}`
- `vended-tools/{bash, edit, read, write, shell, python-repl, mcp-router}/__tests__/*`

Node-only specs out of scope (row with reason): `a2a/server`, `decisions/*`, `mcp/task-client-*`, `sandbox/{docker, ssh}`, `session/file-storage` (deprecated), `vended-memory-stores/bedrock-knowledge-base`, `vended-plugins/context-offloader/*`, `vended-tools/file-editor` (deprecated). Decide and log the scope of `storage/search/qmd` and `vended-memory-stores/test-memory-store`. `*.test.browser.ts` files don't apply.

harness-ts (`harness-ts/test/`): `agent`, `agent-generalist-memory`, `builtin-tools`, `config` (with `fixtures/config-values.ts`), `internal`, `interventions`, `memory`, `models`, `openai-cache`, `prompt`, `sandbox`, `telemetry`, `plugins/{environment, todos}`, `tools/{file-tools, programmatic-tool-caller, shell, subagent, web-fetch, web-search}`; and `integration/{caching, e2e, web-search}` as the live checklist.

Live checklist from strands-ts: `strands-ts/test/integ/` (`agent`, `agent-as-tool`, `agent.cancel`, `interrupt`, `interventions`, `cedar`, `conversation-manager/*`, `mcp/*` minus tasks, `models/*` for in-scope providers, and the rest that are in scope).

## Appendix C: hook registry spike (compiles and runs on stable, edition 2024)

`Cargo.toml` dependencies: `tokio = { version = "1", features = ["full"] }`, `futures = "0.3"`, `async-trait = "0.1"`. This is a spike, not library code: it uses `unwrap` and public fields, which the real implementation must not.

```rust
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, RwLock};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

#[derive(Default)]
pub struct LocalAgent { pub messages: Vec<String> }

pub struct BeforeInvocationEvent<'a> { agent: &'a mut LocalAgent, pub cancel: bool }
impl BeforeInvocationEvent<'_> { pub fn agent_mut(&mut self) -> &mut LocalAgent { self.agent } }

pub struct BeforeToolCallEvent { pub name: String, pub cancel: Option<String> }

// marker-based registry
pub trait HookEvent: 'static {
    type Event<'a>: Send;
}
pub struct BeforeInvocation;
impl HookEvent for BeforeInvocation { type Event<'a> = BeforeInvocationEvent<'a>; }
impl HookEvent for BeforeToolCallEvent { type Event<'a> = BeforeToolCallEvent; }

/// maps a concrete event type (any lifetime) to its marker
pub trait EventKind { type Kind: HookEvent; }
impl<'x> EventKind for BeforeInvocationEvent<'x> { type Kind = BeforeInvocation; }
impl EventKind for BeforeToolCallEvent { type Kind = BeforeToolCallEvent; }

enum Cb<K: HookEvent> {
    Sync(Arc<dyn for<'a, 'b> Fn(&'b mut K::Event<'a>) + Send + Sync>),
    Async(Arc<dyn for<'a, 'b> Fn(&'b mut K::Event<'a>) -> BoxFuture<'b, ()> + Send + Sync>),
}
impl<K: HookEvent> Clone for Cb<K> { fn clone(&self) -> Self { match self { Cb::Sync(f) => Cb::Sync(f.clone()), Cb::Async(f)=>Cb::Async(f.clone()) } } }

#[derive(Default)]
pub struct Registry { bi: RwLock<Vec<Cb<BeforeInvocation>>>, btc: RwLock<Vec<Cb<BeforeToolCallEvent>>> }
pub trait Slot: HookEvent + Sized { fn slot(r: &Registry) -> &RwLock<Vec<Cb<Self>>>; }
impl Slot for BeforeInvocation { fn slot(r: &Registry) -> &RwLock<Vec<Cb<Self>>> { &r.bi } }
impl Slot for BeforeToolCallEvent { fn slot(r: &Registry) -> &RwLock<Vec<Cb<Self>>> { &r.btc } }

pub trait SyncHook<K: HookEvent>: Send + Sync + 'static { fn call(&self, e: &mut K::Event<'_>); }
impl<K: HookEvent, F> SyncHook<K> for F where F: for<'a, 'b> Fn(&'b mut K::Event<'a>) + Send + Sync + 'static { fn call(&self, e: &mut K::Event<'_>) { self(e) } }

#[async_trait::async_trait]
pub trait Hook<E: EventKind + Send>: Send + Sync + 'static { async fn handle(&self, e: &mut E); }

impl Registry {
    // sync form: infer from closure argument type
    pub fn add_hook<M: Slot, F: IntoSyncHook<M>>(&self, f: F) {
        M::slot(self).write().unwrap().push(Cb::Sync(f.into_cb()));
    }
    pub fn add_async_hook<E, F>(&self, f: F) where E: EventKind, E::Kind: Slot, F: for<'a, 'b> Fn(&'b mut <E::Kind as HookEvent>::Event<'a>) -> BoxFuture<'b, ()> + Send + Sync + 'static {
        E::Kind::slot(self).write().unwrap().push(Cb::Async(Arc::new(f)));
    }
    pub fn add_hook_impl<M: Slot, H>(&self, h: H) where H: for<'a> HookImpl<M::Event<'a>> {
        let h = Arc::new(h);
        M::slot(self).write().unwrap().push(Cb::Async(Arc::new(move |e| { let h = h.clone(); Box::pin(async move { h.handle(e).await }) })));
    }
    pub async fn fire<K: Slot>(&self, e: &mut K::Event<'_>) {
        let cbs = K::slot(self).read().unwrap().clone();
        for cb in cbs { match cb { Cb::Sync(f) => f(e), Cb::Async(f) => f(e).await } }
    }
}
/// helper so the closure's annotated arg type drives E inference
pub trait HookArg<E> {}
impl<E, F: Fn(&mut E)> HookArg<E> for F {}

pub struct Agent { local: LocalAgent, reg: Arc<Registry> }
impl Agent {
    pub fn stream(&mut self) -> Pin<Box<dyn futures::Stream<Item = String> + Send + '_>> {
        Box::pin(async_stream_like(self))
    }
}
fn async_stream_like(agent: &mut Agent) -> impl futures::Stream<Item = String> + Send + '_ {
    futures::stream::once(async move {
        let reg = agent.reg.clone();
        let mut ev = BeforeInvocationEvent { agent: &mut agent.local, cancel: false };
        reg.fire::<BeforeInvocation>(&mut ev).await;
        let reg2 = reg.clone();
        let h = tokio::spawn(async move { let mut t = BeforeToolCallEvent { name: "shell".into(), cancel: None }; reg2.fire::<BeforeToolCallEvent>(&mut t).await; t.cancel });
        let c = h.await.unwrap();
        format!("{:?} cancel={} toolcancel={c:?}", agent.local.messages, ev_cancel(&agent.local))
    })
}
fn ev_cancel(_l: &LocalAgent) -> bool { false }

async fn audit(n: &str) { tokio::task::yield_now().await; println!("audit {n}"); }

#[tokio::main]
async fn main() {
    use futures::StreamExt;
    let reg = Arc::new(Registry::default());
    reg.add_hook(|e: &mut BeforeInvocationEvent<'_>| { e.agent_mut().messages.push("sync".into()); e.cancel = true; });
    reg.add_hook(|e: &mut BeforeToolCallEvent| { if e.name == "shell" { e.cancel = Some("no".into()) } });
    reg.add_async_hook::<BeforeInvocationEvent, _>(|e| Box::pin(async move { tokio::task::yield_now().await; e.agent_mut().messages.push("async".into()); }));
    reg.add_async_hook::<BeforeToolCallEvent, _>(|e| Box::pin(async move { audit(&e.name).await; }));
    reg_impl(&reg);
    let mut agent = Agent { local: LocalAgent::default(), reg };
    {
        let mut s = agent.stream();
        while let Some(x) = s.next().await { println!("{x}"); }
    }
    agent.local.messages.push("after".into());
    println!("{:?}", agent.local.messages);
}

pub trait IntoSyncHook<M: HookEvent> { fn into_cb(self) -> Arc<dyn for<'a, 'b> Fn(&'b mut M::Event<'a>) + Send + Sync>; }
impl<F> IntoSyncHook<BeforeInvocation> for F where F: for<'a, 'b> Fn(&'b mut BeforeInvocationEvent<'a>) + Send + Sync + 'static {
    fn into_cb(self) -> Arc<dyn for<'a, 'b> Fn(&'b mut BeforeInvocationEvent<'a>) + Send + Sync> { Arc::new(self) }
}
impl<F> IntoSyncHook<BeforeToolCallEvent> for F where F: for<'b> Fn(&'b mut BeforeToolCallEvent) + Send + Sync + 'static {
    fn into_cb(self) -> Arc<dyn for<'a, 'b> Fn(&'b mut BeforeToolCallEvent) + Send + Sync> { Arc::new(self) }
}

#[async_trait::async_trait]
pub trait HookImpl<E: Send>: Send + Sync + 'static { async fn handle(&self, e: &mut E); }
struct Counter;
#[async_trait::async_trait]
impl<'x> HookImpl<BeforeInvocationEvent<'x>> for Counter { async fn handle(&self, e: &mut BeforeInvocationEvent<'x>) { e.agent_mut().messages.push("impl".into()); } }
pub fn reg_impl(r: &Registry) { r.add_hook_impl::<BeforeInvocation, _>(Counter); }
```

In the real `Agent`, `add_async_hook::<E>` takes `impl Fn` so the turbofish names only the event type, and the registry lives behind `Arc` with ordering, removal and the sealed traits from section 8.5.

## Appendix D: reviewer brief template

> You are an independent reviewer of Phase N of the Strands Rust SDK port. Review only: don't edit files or commit. Repo: `<$REPO>`, branch `feat/rust-sdk`. Phase diff: `git diff <base>..HEAD -- <paths>`. The normative TS spec at pin `7296caba7` is exported at `/tmp/ts-spec/strands-ts/src` (tests in `__tests__`) and `/tmp/ts-spec/harness-ts`. Rules, binding decisions and shapes: sections 5-8, 10 and 11 of `.rust-port/PROMPT.md`. Repo rules: `strands-rs/AGENTS.md` and `strands-rs/docs/TESTING.md`. Pending decisions taken during the run (you may disagree; say so with evidence): `.rust-port/DECISIONS-pending.md`. You may run cargo with `CARGO_TARGET_DIR=/tmp/review-target-<role>`, output redirected to `/tmp/review_<role>_*.txt`. Your role: <correctness | API bar-raiser | parity validator>, focusing on <the role checklist from section 9.9>. Report a numbered list of verified findings, each with `file:line`, what is wrong, the evidence (a TS `file:line`, a failing command, or a compiler error) and the concrete fix. No severity labels, no praise, no restating code. Report only what you verified. End with a one-line verdict.

## Appendix E: implementer brief template

> You are implementing <item> of the Strands Rust SDK. Work autonomously; the owner is unavailable. Read sections 3, 7, 8, 9.2-9.7, 10 and 11 of `.rust-port/PROMPT.md`, then `strands-rs/AGENTS.md` and `strands-rs/docs/TESTING.md`. Repo: `<$REPO>`, branch `feat/rust-sdk` (already checked out; never switch branches). Scope: <plan section text>. TS spec: <paths under /tmp/ts-spec>. Donor: <paths under /tmp/donor-strandly/strands-rs/crates/...>; re-diff against the pin before reusing anything. Decisions that apply: <#s>. Existing code you build on: <modules>; don't duplicate it. Gates: `.rust-port/gates.sh` must pass; run `cargo fmt --all` first. Commit only your paths with `git commit --no-verify` after the secret scan (section 9.5). Append to `.rust-port/DECISIONS-pending.md` (check the highest P number first), `.rust-port/ts-bugs.md`, `DIVERGENCES.md` and `PARITY.md` as needed; never rewrite others' entries. Report back: modules and key public types, test counts from the gate script, TS specs ported and gaps, P numbers logged, TS bugs, anything left undone, and commit hashes.
