# Phase 8 divergences (staging; rows for `strands-rs/docs/DIVERGENCES.md`)

| Area | TS behaviour | Rust behaviour | Reason |
|---|---|---|---|
| `sandbox: false` | On Node, the same as an unset key: the agent runs on the host | `SandboxSource::Off`: `agent.sandbox()` and every tool that needs a sandbox fail with `DefaultNotConfiguredError` ("No Sandbox configured. ...") | The design makes the opt-out explicit (P-264); Rust has no environment without a host default |
| Default sandbox instance | One process-global `NotASandboxLocalEnvironment` registered at import | Each agent without a `sandbox` key builds its own (the type is stateless) | No process-global registration (P-263) |
| Process termination | `SIGTERM`, then `SIGKILL` after 1 s, to the direct child only | The same signals sent to the child's whole process group (job object on Windows); dropping the stream kills the group | Child processes never outlive their tool (P-266) |
| `listFiles` order (local) | `localeCompare` | Lowercase name, then bytes | No ICU collation (P-268) |
| Execution env map | `Record<string, string>` in insertion order | `BTreeMap`, so `export` assignments are in key order | Deterministic; the shell result is the same (P-260) |
