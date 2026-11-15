# Phase 8 divergences (staging; rows for `strands-rs/docs/DIVERGENCES.md`)

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| `sandbox: false` | On Node, the same as an unset key: the agent runs on the host | `SandboxSource::Off`: `agent.sandbox()` and every tool that needs a sandbox fail with `DefaultNotConfiguredError` ("No Sandbox configured. ...") | The design makes the opt-out explicit (P-264); Rust has no environment without a host default |
| Default sandbox instance | One process-global `NotASandboxLocalEnvironment` registered at import | Each agent without a `sandbox` key builds its own (the type is stateless) | No process-global registration (P-263) |
| Process termination | `SIGTERM`, then `SIGKILL` after 1 s, to the direct child only | The same signals sent to the child's whole process group (job object on Windows); dropping the stream kills the group | Child processes never outlive their tool (P-266) |
| `listFiles` order (local) | `localeCompare` | Lowercase name, then bytes | No ICU collation (P-268) |
| Execution env map | `Record<string, string>` in insertion order | `BTreeMap`, so `export` assignments are in key order | Deterministic; the shell result is the same (P-260) |
| `bash` session scope | One module-global map of sessions; killed by `process` exit/SIGINT/SIGTERM handlers | Sessions belong to the tool instance and die with it (or with the agent) | No process-global state or signal handlers (P-270) |
| `bash` stderr | The result is taken when the stdout sentinel arrives; stderr written after that is lost | A second sentinel on stderr; the result waits for both | The command's stderr is always in `error` (P-270) |
| Vended tool validation texts | zod messages | `jsonschema` messages, except the zod custom messages TS tests assert, which are kept | P-51, P-269 |
| `http_request` response body | Read in full | Read as a stream and capped at 10 MiB (`Response body exceeds the 10485760-byte limit: ...`) | Section 11 item 32 (P-272) |
| `http_request` `statusText` | The server's reason phrase | The canonical reason phrase for the status | `reqwest` doesn't expose the received phrase (P-272) |
| `notebook` fractional `insertLine` | Spliced at the truncated index | `Line number out of range` | P-271 |
| `web_fetch` markdown | `turndown` output | `htmd` output with the same options and rules (title heading, dropped elements, safe images and links); whitespace and escaping can differ | No turndown port in Rust; `htmd` follows turndown's rules (P-280) |
| `web_fetch` `maxContentChars` | Counts UTF-16 code units | Counts `char`s | Section 7.3 (P-280) |
| `web_fetch` `statusText` | The server's reason phrase | The canonical reason phrase for the status | `reqwest` doesn't expose the received phrase (P-272, P-280) |
| `subagent` default builder | The child inherits the parent's printer, gets `contextManager: 'auto'` unless its model is stateful, and shares the parent's intervention handlers and trace attributes | The child's printer is off and it gets none of the others | Context managers, interventions and telemetry are not on this branch yet (P-282) |
| `subagent` `maxDepth` | Any positive integer `number` | `u32`, zero rejected | Negative and fractional values are unrepresentable (P-281) |
