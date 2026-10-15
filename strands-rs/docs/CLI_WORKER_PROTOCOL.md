# CLI ⇄ Python worker protocol

Wire contract between the TypeScript CLI (`PythonBackend`) and the Python subprocess that drives an
authored Python agent, specified for re-implementation (e.g. in Rust) from this document alone; every
field is cited to the code that produces or consumes it. Short names: `python.ts`, `worker.py`,
`run.ts`, `acp.ts` = `strands-cli/src/tui/project/<name>`; `types.ts` =
`strands-cli/src/tui/chat/types.ts`; `policy.ts` = `strands-cli/src/tui/permissions/policy.ts`.

The protocol is stateful: the worker owns one `Agent`, serves one turn at a time, and answers each
command with exactly one terminal message (`ready` / `result` / `error`), optionally preceded by
`event` and `authorize` messages.

## 1. Transport

**Spawn** (`python.ts:145-170`): `<python> -u <worker.py> <entrypoint> <root>`.

- Interpreter: `<root>/.venv/bin/python` (`Scripts/python.exe` on win32) when it exists, else
  `python3` (`python.ts:152-158`). `-u` is required — diagnostics must be unbuffered.
- `argv[1]` = entrypoint file, `argv[2]` = project root; both `Path(...).resolve()`d by `main()`
  (`worker.py:782`), from `ImportedAgentProject` (`strands-cli/src/tui/project/import.ts:17-21`).
- cwd = `resolve(options.cwd ?? process.cwd())` (`python.ts:155`, `:159`). The worker never reads a
  `cwd` command field — the process cwd *is* the workspace (`state.session.workspace` is
  `str(Path.cwd())`, `worker.py:423`).
- `stdio: ['pipe','pipe','pipe','pipe']` (`python.ts:165`).

**Descriptors.** fd 0 = CLI → worker JSON-lines commands (`worker.py:783`, `:789`); fd 1/2 = worker
diagnostics only; fd 3 = worker → CLI JSON-lines messages (`worker.py:35`, `python.ts:117`).
One JSON object per line, no ids, no length prefixes. The worker opens fd 3 as
`os.fdopen(3, "w", buffering=1)` and writes `json.dumps(...) + "\n"` (`worker.py:35`, `:52-53`);
default `ensure_ascii=True` guarantees no embedded newlines. The CLI writes
`JSON.stringify(...) + "\n"` (`python.ts:492-496`). Lines on fd 3 must parse as JSON with `type` ∈
`['ready','event','authorize','result','error']`; anything else fails the backend permanently with
`"The Python agent returned an invalid message."` (`python.ts:118-134`). stdout+stderr are kept as a
rolling 8 KiB buffer surfaced only in the child-exit error (`python.ts:112-116`, `:136-141`) — never
parse them.

**`$bytes` convention.** Binary values are wrapped as `{"$bytes": "<base64>"}` both directions:
`Uint8Array` → `{$bytes}` by the stringify replacer (`python.ts:493-495`); Python `bytes` →
`{"$bytes"}` by `json.dumps(default=encode)`, any other non-JSON type raising `TypeError`
(`worker.py:40-43`, `:53`). Decoding: the CLI reviver converts any object with a string `$bytes` to
`Uint8Array` (`python.ts:40-45`, `:120`); the worker's `object_hook` converts an object whose key set
is **exactly** `{"$bytes"}` to `bytes` (`worker.py:46-49`, `:790`). Two exceptions: the **`init` line
is parsed without the hook** (`worker.py:783`), so never send binary in `init`; and
`snapshot`/`stash`/`conversation` payloads travel as JSON *strings* inside `result.finalText`
(`worker.py:613-622`, `:645`, `:649`), so nested `$bytes` are decoded only when that string is itself
parsed (`python.ts:360`, `worker.py:653`, `:658`).

**Environment.** The child inherits the CLI's full `process.env`, plus `PATH` prefixed with the
project venv's bin dir when one was found, and `AWS_DEFAULT_REGION` = `AWS_REGION` when `AWS_REGION`
is set (`python.ts:160-164`). Before spawning, `config.applyProviderEnvironment()` injects detected
provider credentials into the CLI's own env (only where a key is absent/empty), limited to
`AWS_PROFILE`, `AWS_CONFIG_FILE`, `AWS_SHARED_CREDENTIALS_FILE`, `AWS_REGION`, `AWS_DEFAULT_REGION`,
`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_SESSION_TOKEN`, `AWS_BEARER_TOKEN_BEDROCK`,
`ANTHROPIC_API_KEY`, `OPENAI_API_KEY`, `GEMINI_API_KEY`, `OLLAMA_HOST`, `OLLAMA_MODEL`,
`LITELLM_BASE_URL`, `LITELLM_API_KEY`, `LITELLM_MODEL` (`python.ts:156`,
`strands-cli/src/tui/config.ts:245-253`, `strands-cli/src/tui/provider/environment.ts:7-25`, `:49-64`).
**No `STRANDS_*` variable is set for the worker** and `worker.py` reads none — the `STRANDS_CLI_*`
flags are TS-only (`strands-cli/src/logging.ts:44-47`, `strands-cli/src/tui/skills.ts:105`,
`strands-cli/src/tui/workspace/trust.ts:46`, `strands-cli/src/tui/telemetry.ts:25-46`).

## 2. Commands (CLI → worker, fd 0)

Two delivery styles (`python.ts:473-497`):

- **request** `_request(cmd)` — refuses when busy (`'Wait for the Python agent to finish before
  changing it.'`), writes the line, awaits **one** message. `ready`/`result` are accepted (their
  `state` replaces the cached state), `error` throws, and **anything else — including `event` and
  `authorize` — throws `'Unexpected Python response.'`**. A request never drains events; only
  `stream()` does (§6).
- **fire-and-forget** `_send(cmd)` — write only (`python.ts:490-497`).

| `type` | Style | Worker reply | Legal mid-turn |
|---|---|---|---|
| `init` | request (first line) | `ready` | — |
| `prompt` | `_send` + stream drain | `event`*, `authorize`*, then `result` \| `error` | no → busy `error` |
| `permission` | `_send` | none | **yes** |
| `cancel` | `_send` | `result` `"cancelled"` if a turn was running, else nothing | **yes** |
| `close` | `_send` via `stdin.end` | none; worker exits | **yes** |
| `reset` | request | `result` `"reset"` | no |
| `compact` | request | `result` `"compacted"` \| `"unchanged"` | no |
| `skill` | request | `result` `"activated"` | no |
| `snapshot` | request | `result` `"snapshot"` | no |
| `stash` | request | `result` `"stash"` | no |
| `seed` | request | `result` `"seeded"` | no |
| `conversation` | request | `result` `"snapshot"` (capture) \| `"restored"` (restore) | no |
| `reload-failed` | request | `result` `"recorded"` | no |
| other | — | `error` `"Unknown terminal command: <type>"` (`worker.py:696`) | — |

Worker dispatch order: `permission` → `cancel`/`close` → busy check → `prompt` → `Runtime.command()`
(`worker.py:789-810`). While a turn runs, every command except `permission`/`cancel`/`close` —
including a second `prompt` — is answered with `{"type":"error","message":"The Python agent is busy"}`
and dropped (`worker.py:802-803`).

### 2.1 `init`

`{ type: 'init', ...PythonOptions }` — options spread at top level, not nested (`python.ts:176`). Must
be the first line; `main()` reads exactly one line before the command loop (`worker.py:783-786`).
Success ⇒ `ready`; any failure in module load / `Runtime(...)` / `build(...)` ⇒
`{"type":"error","message":str(error)}` then `exit(1)` (`worker.py:819-823`). Unknown extra keys are
ignored.

`PythonOptions` (`python.ts:78-90`), all optional:

| Field | Type | Semantics |
|---|---|---|
| `sourceSelection` | `{ source, configured: {model, thinking}, selected: {model, thinking} }` (`types.ts:215-219`) | Re-applies a chosen model/effort, **only if** `source == module.__file__` and `configured` equals the source's own `{model, thinking}`; then sets `options.model`/`options.effort` from `selected` (`worker.py:241-251`) |
| `overrides` | `Partial<HarnessAgentConfig>` | portable config overrides, §2.2 (`worker.py:212`) |
| `assignments` | `readonly string[]` | `"<dotted.path>=<value>"` merged into `overrides` first; value is `json.loads`-ed, falling back to the raw string; multi-segment paths create/overwrite intermediate dicts (`worker.py:213-231`) |
| `interactive` | `boolean` (default `false`) | installs the `authorize_tool` `BeforeToolCallEvent` hook at `HookOrder.INTERVENTION_INPUT` and registers the `strands_config` tool unless present (`worker.py:266`, `:304-308`). Non-interactive runs never emit `authorize`. Always `true` from `run.ts:48`, `:61`, `:70` and `acp.ts:30-36` |
| `sessionId` | `string` | copied to factory kwarg `session_id`, marked explicit (`worker.py:233-236`) |
| `sessionDir` | `string` | copied to factory kwarg `session_dir`; also seeds `state.session.configured.sessionDir`, default `.agent/sessions` (`worker.py:233-236`, `:252-255`) |
| `sessionStorageDirectory` | `string` | resolved into `Runtime.session_directory`, which is **never read again** — inert, though `fork()` still sends it (`worker.py:256-262`, `python.ts:337`) |
| `mcpServers` | `Record<string, unknown>` | extra servers via `MCPClient.load_servers(servers, continue_on_error=True, prefix_with_server_name=True)`; names already registered as providers (`client_name`) are skipped. Standard `mcpServers` map shape (`worker.py:267`, `:309-319`, `strands-py/src/strands/tools/mcp/mcp_client.py:229-259`) |
| `skillPaths` | `readonly string[]` | appended to an existing `AgentSkills` plugin's sources (re-initialised) or added as `AgentSkills(skills=...)` (`worker.py:268`, `:320-329`) |
| `cwd` | `string` | spawn cwd only; ignored as a command field |
| `resume` | `boolean` | **not read by the worker**; resumption is expressed by `sessionId` + `sessionDir` (set at `run.ts:74`, `acp.ts:35`) |

### 2.2 `overrides` — the `HarnessAgentConfig` subset actually applied

`overrides` is validated by `define_harness_agent_config(partial)`, converted by
`harness_agent_kwargs_from_config(config)`, then **filtered to the keys the caller spelled out**
(`worker.py:106-129`). Accepted keys are exactly the 24 of `DEFAULT_HARNESS_AGENT_CONFIG`
(`harness-py/src/strands_harness/config.py:48-73`; TS mirror `harness-ts/src/config.ts:270-295`);
unknown keys are dropped with a warning (`config.py:88-94`). Mapping is camelCase → snake_case plus
the alias table at `worker.py:115-121`:

| Override key | `create_agent` kwarg | Notes |
|---|---|---|
| `name`, `tools`, `plugins`, `skills`, `memory`, `sandbox`, `session`, `interventions` | same name | `session` = `true`/`false`/`{id, dir}` |
| `description`, `instructions`, `builtinTools`, `builtinPlugins`, `caching` | `description`, `instructions`, `builtin_tools`, `builtin_plugins`, `caching` | force-included even when equal to the default (`worker.py:112-114`) |
| `model` | `model` | string specifier; drives `state.modelSpecifier` |
| `modelModule` | `model` | alias; module reference replaces the string |
| `effort` | `effort` | `'auto'\|'off'\|'minimal'\|'low'\|'medium'\|'high'\|'xhigh'\|'max'` (`harness-ts/src/types/agent.ts:21`) |
| `subagents` | `tools` | alias; each exposed via `as_tool()` and appended (`config.py:168-172`) |
| `mcpServers` | `mcp_servers` | |
| `contextManager` | `context_manager` | |
| `interventionModules` | `interventions` | alias |
| `agentConfig` | *its own keys* | each inner key passes through as a raw kwarg (`worker.py:123`, `:128`) |
| `agentConfigModules` | *its own keys* | same, after module resolution (`worker.py:124`) |
| `memoryStores` | `memory_store` → **dropped** | no such kwarg (stores are folded into `memory`), so the filter at `worker.py:125-129` discards it — send `memory` as well |
| `webFetchModelModule` | `web_fetch_model` → **dropped** | aliased at `worker.py:118` but not a valid config key, so validation removes it first |
| `dependencies` | — | explicitly excluded (`worker.py:128`) |

`sessionId`/`sessionDir` are injected as `session_id`/`session_dir` *after* this mapping
(`worker.py:233-235`); the worker does not translate them into `session={"id","dir"}`, so the
project's `create_agent(**overrides)` must accept them. The explicit key set is remembered
(`worker.py:236`) and re-sent to `create_agent` on every rebuild (`worker.py:274`). A module that
exports an `agent` instance with no explicit keys is used as-is; otherwise `create_agent` must exist
or `init` fails with `"This source exports an agent instance but not create_agent(**overrides), …"`
(`worker.py:275-285`).

### 2.3 `prompt`, `permission`, `cancel`, `close`

- `{ "type": "prompt", "prompt": "<string>" | [<ContentBlockData>…] }` (`python.ts:222-227`; blocks per
  §7). Fire-and-forget; the reply stream is `event`*/`authorize`* terminated by exactly one `result`
  or `error`.
- `{ "type": "permission", "requestId": "<string>", "approved": <bool> }` (`python.ts:243`, `:398`)
  resolves the pending future under `requestId`; unknown or already-resolved ids are silently ignored
  (`worker.py:792-795`). Legal at any time; serves both authorization flavours (§5).
- `{"type":"cancel"}` / `{"type":"close"}` both set `runtime.cancel_signal` and cancel the running
  turn task; `close` also breaks the loop, awaits the cancelled task, runs `runtime.cleanup(agent)`
  (memory flush + `agent.cleanup()`) and exits (`worker.py:796-801`, `:811-816`). `close` is written
  with EOF: `stdin.end('{"type":"close"}\n')` (`python.ts:420`).

### 2.4 `reset`

`{ "type": "reset", "model"?: string, "thinking"?: string|null, "sessionId"?: string, "clear"?: true }`
(`python.ts:302`, `:312`, `:317`; `worker.py:661-674`).

- `model` → `options["model"]`; `thinking` → `options["effort"]` (`null` means "none",
  `python.ts:312`); `sessionId` → `options["session_id"]` **only when** `options.get("session", True)`
  is truthy. Each marks the matching explicit key.
- `clear` truthy ⇒ rebuild with no snapshot (fresh conversation); otherwise the current
  `take_snapshot(preset="session")` is reloaded into the new agent.
- Reply `result` `"reset"`. Callers: model switch (`modelChangeMode()` is `'restart'`), effort change,
  and `clear()` which pairs `clear: true` with a fresh `sessionId` UUID.

### 2.5 `compact`

`{ "type": "compact" }` (`python.ts:321`; `worker.py:675-680`, `:469-491`). ≤2 messages ⇒ `result`
`"unchanged"` (CLI maps to `undefined`, `python.ts:322`). Otherwise
`SummarizingConversationManager(summary_ratio=0.8, preserve_recent_messages=2)` runs; an unchanged
message list raises ⇒ `error` `"The model did not return a usable summary. Try /compact again."`.
Success ⇒ `result` `"compacted"` with `context: { currentTokens?, contextWindow? }` (`None` keys
stripped).

### 2.6 `skill`

`{ "type": "skill", "name": "<skill name>" }` (`python.ts:326`; `worker.py:681-694`). Activates via the
`AgentSkills` plugin's `skills` tool, saves, replies `result` `"activated"`. No skills plugin ⇒
`error` `"This agent has no skills"`.

### 2.7 `snapshot` / `stash` / `seed` (fork)

`fork()` opens a second worker then runs (`python.ts:330-356`):

1. `{"type":"snapshot"}` → `result` `"snapshot"`, `finalText` = `json.dumps` of
   `agent.take_snapshot(preset="session").to_dict()` — **snake_case** keys (§7) (`worker.py:643-645`).
2. `{"type":"stash"}` → `result` `"stash"`, `finalText` = `json.dumps` of
   `context_manager.stash.take_snapshot()`, a `{"<ref key>": <JSONValue>}` map, `{}` when the agent has
   no stash (`worker.py:646-649`, `strands-py/src/strands/_context_manager/stash.py:149-161`).
3. On the target: `{ "type": "seed", "snapshot": "<string>", "stash": "<string>" }` — the two
   `finalText` strings verbatim. `snapshot` is required and parsed with
   `Snapshot.from_dict(json.loads(…, object_hook=decode))`; `stash` is applied only when truthy and
   the agent has a stash. Reply `result` `"seeded"` (`worker.py:650-660`).

The fork's `init` carries a fresh `sessionId` UUID, the source's `sessionDirectory` as
`sessionStorageDirectory`, its `cwd`, and `overrides.effort`/`overrides.model` from its live state
(`python.ts:330-346`).

### 2.8 `conversation`

Capture — `{"type":"conversation"}` (`python.ts:358-361`; `worker.py:593`, `:608-624`) ⇒ `result`
`"snapshot"` whose `finalText` is a JSON string with **camelCase** keys
(`{scope, schemaVersion, createdAt, data, appData}`), built from
`take_snapshot(include=["messages","state","conversation_manager_state"])`. The CLI parses it with the
`$bytes` reviver (`python.ts:360`).

Restore — `{ "type": "conversation", "snapshot": <Snapshot camelCase>, "preserveReasoning": <bool> }`
(`python.ts:367-373`; `worker.py:593-607`). Only `snapshot.data` and `snapshot.schemaVersion` are read;
the worker rebuilds `Snapshot(scope="agent", schema_version=…, data=data, app_data={})`. Falsy
`preserveReasoning` strips every `reasoningContent` block from `data["messages"]` and drops messages
left empty; the CLI sets it to `conversation.model === this.info().model` (`python.ts:371`). Reply
`result` `"restored"`.

### 2.9 `reload-failed`

`{ "type": "reload-failed", "message": "<string>" }` (`python.ts:266`; `worker.py:625-642`). Appends a
user message reporting the failed source reload (including `message`), saves, replies `result`
`"recorded"`. Sent via the `SetupChange.onFailure` callback from `takeReloadRequest()`.

## 3. Messages (worker → CLI, fd 3)

`WorkerMessage` union: `python.ts:64-76`.

**`ready`** — `{ "type": "ready", "state": <PythonState> }`, exactly once after `init`
(`worker.py:786`); `_request` converts it to the synthetic `{ stopReason: 'ready' }` (`python.ts:482`).

**`event`** — `{ "type": "event", "event": <ChatEvent> }` (`worker.py:56-57`), only during a turn (§4).

**`authorize`** — `{ "type":"authorize", "id", "name", "input", "pathInWorkspace", "before"? }`
(`worker.py:68-90` + the `id` added by `permission()`, `:95-101`):

| Field | Type | Meaning |
|---|---|---|
| `id` | `string` | `str(uuid4())`, minted per request; echo back as `permission.requestId` |
| `name` | `string` | `tool_use["name"]`; `strands_config` is never authorized (early return, `worker.py:70-71`) |
| `input` | `Record<string, JSONValue>` | `tool_use.get("input", {})` |
| `pathInWorkspace` | `boolean` | `Path(input["path"]).resolve().is_relative_to(Path.cwd().resolve())` when `input.path` is a string, else `false` (`worker.py:76-82`) |
| `before` | `string?` | for `write`/`edit` with a string path: current file text via `agent.sandbox.read_text(path)`, or `""` when missing and the tool is `write`; absent on any other error (`worker.py:83-89`) |

**`result`** — `{ "type":"result", "result": <ChatRunResult>, "state": <PythonState>, "reload": <bool> }`
(`worker.py:454-455`). `reload` is always present but only meaningful after a `prompt`: `true` only
when the agent called `strands_config(action="apply")` during the turn and the turn was not cancelled
(`worker.py:356`, `:552`). The CLI records it from `stream()` only (`python.ts:237`); `_request`
ignores it. `takeReloadRequest()` then yields a `SetupChange` whose `onFailure` sends `reload-failed`
(`python.ts:260-269`).

**`error`** — `{ "type": "error", "message": "<string>" }`: busy rejection (`worker.py:803`), any
exception out of `Runtime.command()` (`:809-810`), a failed turn after session rollback (`:580-587`),
or top-level startup failure followed by `exit(1)` (`:819-823`). The CLI throws it with the message
run through `sanitizeTerminalText` (`python.ts:230-233`, `:479`).

### 3.1 `PythonState`

TS shape `python.ts:47-62`; producer `Runtime.state()` (`worker.py:381-452`). Carried by every `ready`
and `result`, and cached as the backend's view of the agent.

| Field | Type | Computation |
|---|---|---|
| `name` | `string` | `agent.name or "Agent"` (`:396`) |
| `reconstructable` | `boolean` | `callable(self.factory)` — module exports `create_agent`, or a direct `agent = create_harness(...)` assignment was captured (`:397`, `:769-777`) |
| `description` | `string` | `agent.description or ""` (`:398`) |
| `model` | `string` | `options["model"]` if a string, else `agent.model.get_config().get("model_id", type(model).__name__)` (`:382-384`) |
| `modelSpecifier` | `string?` | only when `options["model"]` is a string (`:400`); what `restartModel`/`fork` compare and re-send |
| `thinking` | `string \| boolean \| null` | `options.get("effort", "auto")` (`:401`); TS `Exclude<EffortInput, undefined>` (`strands-cli/src/tui/model/selection.ts:19`) |
| `sourceSelection` | object? | only when the source configured a string model: `{ source: module.__file__, configured: {model, thinking}, selected: { model: options.model or DEFAULT_MODEL, thinking: options.effort or "auto" } }` (`:238-240`, `:402-415`) |
| `sessionId` | `string?` | `SnapshotSessionManager.session_id` when that manager is in use, else `options.get("session_id")`; omitted when falsy (`:386-388`, `:416`) |
| `sessionDirectory` | `string` | `local_session_directory(storage)` — unwraps nested prefixed storages to a `LocalFileStorage` and joins `_base_dir` + prefixes — else the resolved `options["session_dir"]` (`:175-182`, `:257`, `:390`, `:417`) |
| `session` | object? | only when the session dir is local: `{ sessionId, sessionDirectory, workspace: str(Path.cwd()), configured: { sessionId: <init sessionId or null>, sessionDir: <init sessionDir or ".agent/sessions"> } }` (`:252-255`, `:418-429`); matches `types.ts:209-214` |
| `messages` | `MessageData[]` | `display_messages(agent)`: `text` and `toolUse` blocks verbatim; a `toolResult` keeps its text/json content and replaces anything else with `{"text":"[media]"}`; messages with no surviving block are dropped. Reasoning/image/document blocks, `trackingId` and `metadata` are **not** sent (`:150-172`) |
| `privatePaths` | `string[]` | session dir (if local) + stash dir + every `ContextOffloader` storage dir + resolved `options.get("memory_dir", ".agent/memory")` (`:189-206`, `:431-436`) |
| `tools` | `{ name, description }[]` | `agent.tool_registry.get_all_tool_specs()` (`:437-440`) |
| `skills` | `SkillInfo[]` | `{ name, description, instructions, active }` per available skill, `active` = name ∈ `get_activated_skills(agent)`; `[]` with no skills plugin (`:441-451`). TS `SkillInfo` also allows `path`, `allowedTools`, `license`, `compatibility` (`strands-cli/src/tui/skills.ts:10-18`), none of which the worker sends |

The CLI exposes `state.messages` as SDK messages via `Message.fromJSON` (`python.ts:202-204`).

## 4. `ChatEvent`s and `ChatRunResult`

Union: `types.ts:137-181`. The worker emits five of the nine variants (`worker.py:501-525`, `:560-573`)
— never `toolOutputDelta`, `tasks`, `context`, and never a `video` media block:

```ts
| { type: 'reasoningDelta'; text: string }                                  // types.ts:138-141
| { type: 'textDelta'; text: string }                                       // types.ts:142-145
| { type: 'media'; content: ChatMediaContent }                              // types.ts:146-149
| { type: 'toolStart'; toolUseId: string; name: string; input: JSONValue; background?: boolean }   // types.ts:150-156
| { type: 'toolResult'; toolUseId: string; status: 'success' | 'error';
    content: readonly ChatToolResultContent[]; error?: string }             // types.ts:163-169
| { type: 'permission'; request: ChatPermissionRequest }                    // types.ts:178-181
```

| Event | Trigger | Wire shape |
|---|---|---|
| `textDelta` | string `event["data"]` | `{type:"textDelta", text}` (`:502-503`) |
| `reasoningDelta` | truthy `event["reasoningText"]` | `{type:"reasoningDelta", text}` (`:504-505`) |
| `toolStart` | `toolUse` block in `event["message"]["content"]` | `{type:"toolStart", **toolUse}` ⇒ `toolUseId`, `name`, `input` (+ any extra block key); `background` never set (`:508-509`) |
| `toolResult` | `event["tool_result"]` or a `toolResult` block, de-duplicated by `toolUseId` per turn | `{type:"toolResult", toolUseId, status, content:[display_content(b)…]}`; `error` never set (`:506`, `:510-525`) |
| `media` | `image`/`document` block on a message | `{type:"media", content: display_content(block)}` (`:512-513`) |
| `permission` | each `Interrupt` of a `stop_reason == "interrupt"` result | §5.2 (`:556-573`) |

`display_content` (`worker.py:132-147`) produces `ChatToolResultContent`/`ChatMediaContent`
(`types.ts:106-135`): `{"text":…}` → `{type:"text", text}`; `{"json":…}` → `{type:"json", value}`;
`{"image"|"document": media}` → `{type:<kind>, …media minus source…, source:{type:<k>, <k>:<value>}}`
where `<k>` is the SDK source's single key (`bytes`, `s3`, `url`, `text`, `content`) — `bytes` is
`$bytes`-wrapped, giving `{type:'image', format, source:{type:'bytes', bytes: Uint8Array}}`
(`types.ts:95-104`); anything else → `{type:"text", text: str(block)}`.

```ts
export interface ChatRunResult {   // types.ts:183-191
  stopReason: string
  usage?: TokenUsage
  watchUsage?: (listener: (usage: TokenUsage | undefined) => void) => () => void
  context?: ChatContextUsage
  finalText?: string
  finalReasoning?: string
}
```

`TokenUsage` = `{ inputTokens?, outputTokens?, totalTokens, cacheReadInputTokens, cacheWriteInputTokens }`
(`types.ts:27-33`); `ChatContextUsage` = `Partial<TokenUsage> & { currentTokens?, projectedTokens?, contextWindow? }`
(`types.ts:35-39`). `watchUsage`/`finalReasoning` are TS-only, never on the wire. A completed turn
(`worker.py:536-553`):

```json
{ "stopReason": "end_turn", "finalText": "<str(AgentResult)>",
  "context": { "currentTokens": 1234, "projectedTokens": 1300, "contextWindow": 200000 },
  "usage": { "cacheReadInputTokens": 0, "cacheWriteInputTokens": 0,
             "inputTokens": 10, "outputTokens": 20, "totalTokens": 30 } }
```

- `stopReason` from `AgentResult.stop_reason` ∈ `cancelled`, `checkpoint`, `content_filtered`,
  `end_turn`, `guardrail_intervened`, `interrupt`, `limit_output_tokens`, `limit_total_tokens`,
  `limit_turns`, `max_tokens`, `stop_sequence`, `tool_use`
  (`strands-py/src/strands/types/event_loop.py:39-52`); `interrupt` never reaches the CLI (§5.2).
  Worker-synthesized: `cancelled`, `snapshot`, `stash`, `seeded`, `restored`, `recorded`, `reset`,
  `compacted`, `unchanged`, `activated`; plus the CLI-side `ready` (`python.ts:482`).
- `finalText` = `str(result)`: structured output JSON else concatenated text blocks
  (`strands-py/src/strands/agent/agent_result.py:64-76`). `context` keys: `result.context_size`,
  `result.projected_context_size`, `agent.model.context_window_limit`; `None` dropped. `usage` =
  `result.metrics.accumulated_usage` with both cache counters defaulted to `0`
  (`strands-py/src/strands/types/event_loop.py:8-23`).
- A cancelled turn sends **only** `{"stopReason":"cancelled"}` — no `usage`/`context`/`finalText`
  (`worker.py:578-579`).

## 5. Authorization flow

Two mechanisms share the `permission` command and the worker's `pending_permissions: dict[str, Future]`
(`worker.py:36`, `:95-103`).

### 5.1 `authorize` — pre-tool-call gate (interactive only)

1. The `authorize_tool` hook sends `authorize` and awaits the future keyed by its `id`
   (`worker.py:68-90`); the turn blocks and no `result` is sent.
2. `_authorize` decides (`python.ts:436-471`): `permissions.mode === 'bypassPermissions'` or the tool
   in the configured allow-list ⇒ approve. Else Cedar `isAuthorized` with principal
   `User::"anonymous"`, action `Action::"<name>"`, resource `Resource::"agent"`, no entities, and
   `context = { input: <authorize.input>, session: { path_in_workspace: authorize.pathInWorkspace && name === 'read' && pathIsWithinWorkspace(input.path, cwd) } }`
   against `DEFAULT_CEDAR_POLICIES` (`policy.ts:12-47`: unconditional permits for `todo_write`,
   `retrieve_context`, `retrieve_offloaded_content`; `read` only when
   `context.session.path_in_workspace`; `strands_config` requires `context.session.configuration_draft`,
   which this context never sets). Cedar `type === 'failure'` throws
   `'Cedar permission evaluation failed.'`, aborting the stream. `decision === 'allow'` ⇒ approve.
3. Otherwise prompt via `ToolPermissionBroker.request(name, input, diff)` — a
   `ChatPermissionRequest` with broker-local id `strands-permission-<n>` and options `allow-once` /
   `allow-tool-always` / `deny` (`policy.ts:49-96`). `diff` is a TS-only preview derived from
   `authorize.before` (`write` → `input.content`; `edit` → `before.replace(old_str, new_str)`), never
   sent back (`python.ts:455-467`). `allow-tool-always` persists the tool in CLI config; approved =
   `decision !== 'deny'`.
4. The CLI replies `{ type:'permission', requestId: <authorize.id>, approved }` (`python.ts:243`).
   `approved === false` ⇒ the worker sets
   `event.cancel_tool = "DENIED: Tool call denied by the user"` and the agent sees a denial tool result
   (`worker.py:91-92`).

Broker ids never go on the wire; only `authorize.id` does.

### 5.2 `permission` event — SDK interrupts

For a turn result with `stop_reason == "interrupt"` and interrupts, per interrupt
(`worker.py:555-577`): register a future under `interrupt.id`, then emit

```json
{ "type": "event",
  "event": { "type": "permission",
             "request": { "id": "<interrupt.id>", "toolName": "<interrupt.name>",
                          "input": <interrupt.reason>,
                          "options": [ { "id": "allow", "label": "Allow", "kind": "allow_once" },
                                       { "id": "deny",  "label": "Deny",  "kind": "reject_once" } ] } } }
```

(`ChatPermissionRequest` `types.ts:73-79`, `ChatPermissionOption` `types.ts:53-58`, `Interrupt`
`strands-py/src/strands/interrupt.py:16-31`.) Nothing answers it automatically:
`respondPermission(id, optionId)` first tries the broker (which rejects the unknown id) and then sends
`{ type:'permission', requestId: id, approved: optionId === 'allow' }` (`python.ts:396-400`). Console
mode asks on the terminal (`run.ts:111-117`); ACP forwards `session/request_permission`
(`acp.ts:67-82`). The boolean becomes the interrupt response and the worker re-enters the loop with
`prompt = [{ "interruptResponse": { "interruptId": <id>, "response": <bool> } }, …]` (`worker.py:575`,
`strands-py/src/strands/types/interrupt.py:126-145`).

### 5.3 Interaction with `cancel`, `reset`, failures

- `cancel()` calls `_permissions.cancelAll()` first, resolving every queued/active prompt as `'deny'`
  (so an in-flight `authorize` is answered `approved: false`), then sends `cancel`
  (`python.ts:402-413`, `policy.ts:125-133`). `setPermissionMode` and `_fail` also `cancelAll()`
  (`python.ts:385`, `:501`), so a backend failure denies rather than hangs.
- In the worker, cancelling the turn raises `CancelledError` inside the awaited future; each
  `pending_permissions` entry is popped in its `finally` (`worker.py:102-103`, `:576-577`), and a
  `permission` arriving later finds no future and is ignored (`:793-795`).
- `reset` can never race a pending authorization: `_request` throws while `_running`
  (`python.ts:474`), and a `reset` that did reach a busy worker gets the busy `error`
  (`worker.py:802-803`).

## 6. Sequencing, concurrency, lifecycle

**One turn at a time.** CLI: `stream()` throws `'The Python agent is busy.'` when `_running`
(`python.ts:223`), `_request` throws `'Wait for the Python agent to finish before changing it.'`
(`:474`); `_running` is cleared in `finally` on both paths. Worker: busy `error` for everything but
`permission`/`cancel`/`close` (`worker.py:802-803`).

**Cancel mid-turn.** `cancel()` denies pending prompts, sends `{type:'cancel'}`, and — if `_running`
with no timer armed — starts a **3 s** `_cancelTimeout`; on expiry the backend fails with `'The Python
agent did not stop after cancellation. Reload it to continue.'` and disposes itself
(`python.ts:402-413`). The timer is cleared whenever a turn or request finishes (`python.ts:255-256`,
`:485-486`). The worker sets `cancel_signal` (also passed to
`agent.stream_async(..., cancel_signal=…)`, `worker.py:501`) and cancels the task, which replies
`result` `{"stopReason":"cancelled"}` with fresh state (`:578-579`). A `cancel` while idle only sets
the signal — no reply; the next turn clears it (`:494`).

**Close/exit.** `dispose()` is idempotent: clear the timer, dispose the broker,
`stdin.end('{"type":"close"}\n')`, wait up to **1 s** for the child's `close`, `terminateProcessTree`
if still alive, then `_fail(new Error('Python agent closed.'))` (`python.ts:415-434`). The worker also
exits on stdin EOF (`while line := …readline()`) with the same cleanup (`worker.py:789`, `:811-816`).
Child `close` ⇒ `_fail(new Error('Python agent exited (<signal ?? code>). <diagnostics>'))` and the
fd-3 reader closes (`python.ts:136-141`).

**Message pump.** `_messages` is a FIFO; `_next()` awaits a wake-up when empty and rethrows the sticky
`_failure` rather than blocking (`python.ts:505-514`). `_failure` is set once and makes every later
`_send` throw (`:490-491`, `:499-503`). `stream()` loops `_next()`: `error` ⇒ clear `_reloadRequested`,
mark completed, throw; `result` ⇒ adopt `state`, record `reload`, return; `authorize` ⇒ resolve and
reply `permission`; `event` ⇒ yield (`:228-247`). Abandoning the generator before the terminal message
triggers `cancel()` **and** `dispose()` in `finally`, so a later prompt can never consume a dead
turn's queued events (`:248-257`). `_request` does not tolerate `event`/`authorize` (§2) — only issue
requests when idle.

**Open.** `PythonBackend.open` arms a **60 s** timeout and an `AbortSignal` listener, both of which
`_fail` the backend (`'Python agent loading timed out.'` / `'Python agent loading cancelled.'`), awaits
`_request({type:'init',…})`, and disposes the child on any throw (`python.ts:171-185`).

## 7. Message / ContentBlock / Snapshot JSON

`state.messages` is the SDK's serialized form (`strands-ts/src/types/messages.ts:35-56`):

```ts
interface MessageData { role: Role; content: ContentBlockData[]; trackingId?: string; metadata?: MessageMetadata }
type Role = 'user' | 'assistant'                                     // messages.ts:163
type ContentBlockData =                                              // messages.ts:178-189
  | TextBlockData | { toolUse: ToolUseBlockData } | { toolResult: ToolResultBlockData }
  | { reasoning: … } | { cachePoint: … } | { guardContent: … }
  | { audio: … } | { image: … } | { video: … } | { document: … } | { citations: … }
```

`ToolUseBlockData` = `{ name, toolUseId, input }` (`messages.ts:254-266`); `ToolResultBlockData` =
`{ toolUseId, status: 'success'|'error', content: ToolResultContentData[] }` (`messages.ts:362-376`).
Only `text`/`toolUse`/`toolResult` appear in `state.messages` (`worker.py:150-172`); the CLI rehydrates
with `Message.fromJSON` (`python.ts:203`, `messages.ts:147-149`). Prompts use the same vocabulary
(`stream(prompt: string | ContentBlockData[])`, `python.ts:222`), with `Uint8Array` fields (e.g.
`image.source.bytes`) `$bytes`-encoded.

Snapshots come in two spellings — the one real trap here:

| Carrier | Keys | Source |
|---|---|---|
| `conversation` capture/restore | `scope`, `schemaVersion`, `createdAt`, `data`, `appData` | hand-built camelCase (`worker.py:613-622`), equal to TS `Snapshot` (`strands-ts/src/types/snapshot.ts:20-31`) and `ChatConversation['snapshot']` (`types.ts:205-206`) |
| `snapshot` command / `seed` | `scope`, `schema_version`, `created_at`, `data`, `app_data` | `Snapshot.to_dict()` / `from_dict()` (`strands-py/src/strands/types/_snapshot.py:75-100`) |

`schema_version` must be `"1.0"` and `scope` ∈ `{"agent","multiAgent"}` or `from_dict` raises
(`_snapshot.py:34`, `:61-73`, `:92-100`); it also indexes `created_at` and `data` directly (both
required). `data` carries the preset's fields — `preset="session"` ⇒ `messages`, `state`,
`conversation_manager_state`, `interrupt_state`, `model_state` (`_snapshot.py:36-38`), narrowed by
`conversation` capture to the first three (`worker.py:609`).

## 8. Porting gotchas

1. A request that races an `event`/`authorize` is a CLI-side protocol error — serialize strictly.
2. Cancellation yields `result` `"cancelled"`, not `error`; cancelling while idle yields nothing.
3. `$bytes` decode is key-set-exact in Python, key-presence-based in TS; `init` skips the hook.
4. `sessionStorageDirectory`, `cwd`, `resume`, `overrides.memoryStores` and
   `overrides.webFetchModelModule` are accepted but change nothing.
5. `state.messages` is lossy — use `conversation`/`snapshot` when something must round-trip.
