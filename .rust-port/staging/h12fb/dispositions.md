# Dispositions staged by track h12fb

| finding | disposition | commit |
|---|---|---|
| correctness-9 | Fixed in the SDK. Reproduced first: with the old code the new test saw `["add"]` in the first request instead of `["late"]`. `invoke_model_with_middleware` now reads `tool_registry().tool_specs()` when it builds the model call, after the `BeforeModelCallEvent` hooks and on every attempt, as TS `_invokeModelWithMiddleware` does (`agent.ts:2600`). The token estimate keeps the read from the top of `invoke_model`, as TS does (`agent.ts:2397,2414,2474`), so `projected_input_tokens` and the context manager's proactive compression are unchanged; the structured-output tool is registered before the loop and is in both reads. Test: `agent::run::tests::hooks::a_registry_change_in_a_before_model_call_hook_reaches_that_calls_tool_specs`. P-1131. | 38d1c539e |
