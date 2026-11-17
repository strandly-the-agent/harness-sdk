# Dispositions written by the lead (findings closed by another finding's fix)

| finding | disposition | commit |
|---|---|---|
| api-4 | Fixed with correctness-8: `shutdown_telemetry` is public, re-exported from the crate root with or without the `otel` feature, and documented on `create_harness`, in the crate docs and the README (P-1130, closing P-923). | 744809cfe |
| api-7 | Fixed with correctness-4: `auth` values other than `clientId` and `scopes` are redacted, with `env` and `headers`, in every `Debug` reachable from `HarnessAgentOptions` and `HarnessAgentConfig`. | c442db12a |
| api-10 | Fixed before the report was written: `GENERALIST`, `GENERALIST_INSTRUCTIONS` and `CONTEXT_MODES` are `pub(crate)` and the `make_subagent` doc example builds its own `Preset` (P-1110, closing P-993). | 818341af7 |
| api-15 | Fixed with correctness-3: a consumer `Todos` or `EnvironmentContext` under its own name stands in for the built-in one (recognised by name or by the tool specs it vends; a turn that already has an `<environment>` block gets no second one), so the field doc now holds (P-1142). | ccdb5dbe5, 3809ab8ae |
| parity-2 | Fixed with correctness-8 (see api-4); the divergence row "Flushing spans at exit" lists the difference. | 744809cfe |
| parity-7 | Fixed with correctness-2: the portable `agentConfig` keys that have a JSON form are applied with TS precedence, the others are errors, and the divergence row states what remains (P-1120). | 968c95d44 |
| parity-3 | Fixed with api-3: `Effort` parses through the public `FromStr`, the mapped test `rejects_an_effort_outside_the_enum_by_name` calls it, and the divergence row names the reachable path. | 57dc5aae8 |
| parity-4 | Fixed with api-16: the stale `expect(dead_code)` markers and the four unread default constants are gone. | 17411ae36 |
