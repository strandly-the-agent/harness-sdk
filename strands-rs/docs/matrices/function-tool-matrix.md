# `function-tool` module — TS → Rust test matrix

Module: `strands-rs/crates/strands-agents/src/tools/{function_tool,noop_tool,structured_output}.rs`
(+ test additions in `tools/tool.rs`, `#[cfg(test)] test_agent()` in `agent/mod.rs`).

Counts: **71 ported**, **1 merged**, **6 skipped (not portable)**, 0 failing.
All Rust tests live in `#[cfg(test)] mod tests` next to the code.

## `tools/__tests__/tool.test.ts`

### isValidToolName
| TS test | Rust test | Status |
|---|---|---|
| accepts `simple`/`with_underscore`/`with-hyphen`/`Mixed-Case_123`/`a`/64 chars | `tool.rs::name_and_event_tests::accepts_valid_tool_names` | pass |
| rejects empty/65 chars/space/dot/slash/colon/non-ascii | `tool.rs::name_and_event_tests::rejects_invalid_tool_names` | pass |

### FunctionTool / properties
| TS test | Rust test | Status |
|---|---|---|
| has a non-empty toolName | `has_a_non_empty_tool_name` | pass |
| has a non-empty description | `has_a_non_empty_description` | pass |
| has a valid toolSpec | `has_a_valid_tool_spec` | pass |
| has matching toolName and toolSpec.name | `has_matching_tool_name_and_tool_spec_name` | pass |
| has matching description and toolSpec.description | `has_matching_description_and_tool_spec_description` | pass |

### stream / synchronous callback
| TS test | Rust test | Status |
|---|---|---|
| wraps return value in ToolResult | `wraps_return_value_in_tool_result` | pass |
| handles string return values | `handles_string_return_values` | pass |
| handles object return values | `handles_object_return_values` | pass |
| treats objects with extra keys beyond a content block key as JSON | `treats_objects_with_extra_keys_beyond_a_content_block_key_as_json` | pass |
| passes input to callback exactly as provided to stream | `passes_input_to_callback_exactly_as_provided_to_stream` | pass |
| handles null return values correctly | `handles_null_return_values_correctly` | pass |
| handles undefined return values correctly | `handles_undefined_return_values_correctly` (`Ok(())` → JSON `null`) | pass |
| handles boolean return values as text content | `handles_boolean_return_values_as_text_content` | pass |
| handles number return values as text content | `handles_number_return_values_as_text_content` | pass |
| handles array return values as wrapped JSON content | `handles_array_return_values_as_wrapped_json_content` | pass |
| deep copies objects to prevent mutation | — | **skipped**: `JsonValue` is moved into the result; no shared mutable reference exists in Rust. |
| deep copies arrays to prevent mutation | — | **skipped**: same reason. |
| *(no TS test; TS rule "arrays of content blocks → content array")* | `arrays_of_content_blocks_become_the_content_array` | pass (added) |
| *(no TS test; `_asToolResultContent` single-key path)* | `single_key_content_blocks_pass_through` | pass (added) |

### stream / promise callback
| TS test | Rust test | Status |
|---|---|---|
| wraps resolved value in ToolResult | `wraps_resolved_value_in_tool_result` | pass |
| can access ToolContext in promise | `can_access_tool_context_in_promise` | pass |

### stream / async generator callback → `ctx.progress()`
| TS test | Rust test | Status |
|---|---|---|
| yields ToolStreamEvents then final ToolResult | `yields_tool_stream_events_then_final_tool_result` | pass |
| can yield objects as ToolStreamEvents | `can_yield_objects_as_tool_stream_events` | pass |

### stream / error handling
| TS test | Rust test | Status |
|---|---|---|
| catches synchronous errors | `catches_synchronous_errors` | pass |
| catches promise rejections | `catches_promise_rejections` | pass |
| captures Error object in ToolResult when callback throws Error | `captures_error_object_in_tool_result_when_callback_throws_error` | pass |
| wraps non-Error thrown values into Error object | `wraps_non_error_thrown_values_into_error_object` | pass (handlers can only return `Error`; asserts the `Error: <msg>` text + attached error) |
| preserves custom Error subclass instances | `preserves_custom_error_subclass_instances` (downcasts through `Error::Other`) | pass |
| preserves error stack traces | — | **skipped**: Rust errors carry no stack. |
| captures errors thrown in async generator callbacks | `captures_errors_thrown_in_async_generator_callbacks` | pass |
| catches errors in async generators | `catches_errors_in_async_generators` | pass |
| handles non-Error thrown values | `handles_non_error_thrown_values` | pass |
| returns error for circular references | `returns_error_for_unserializable_values` (map with a tuple key → `serde_json` failure → error result) | pass (nearest analogue) |
| silently drops non-serializable values (functions) | — | **skipped**: no function values in `JsonValue`/`Serialize`. |
| *(spec §6.3: `InterruptError` rethrown)* | `propagates_cancellation` (`Error::Cancelled` propagates as `Err`) | pass (added) |
| *(TS `invoke()`)* | `invoke_raw_returns_the_raw_value_and_propagates_errors` | pass |

### Tool interface backwards compatibility / instanceof
| TS test | Rust test | Status |
|---|---|---|
| maintains stable interface signature | `maintains_stable_interface_signature` (via `ToolRef`) | pass |
| stream method accepts correct parameter types | merged into `maintains_stable_interface_signature` | merged |
| FunctionTool passes instanceof Tool check | covered by `maintains_stable_interface_signature` / `typed_tool_is_usable_as_a_tool_ref` | pass |
| can be used as type guard | — | **skipped**: no runtime type guards in Rust. |

### ToolStreamEvent
| TS test | Rust test | Status |
|---|---|---|
| creates instance with data | `tool.rs::name_and_event_tests::creates_tool_stream_event_with_data` | pass |
| creates instance without data | `tool.rs::name_and_event_tests::creates_tool_stream_event_without_data` | pass |
| creates instance with structured data | `tool.rs::name_and_event_tests::creates_tool_stream_event_with_structured_data` | pass |

### optional inputSchema
| TS test | Rust test | Status |
|---|---|---|
| creates tool with default empty object schema | `creates_tool_with_default_empty_object_schema` | pass |
| executes successfully with empty input | `executes_successfully_with_empty_input` | pass |
| callback receives empty object when no schema provided | `callback_receives_empty_object_when_no_schema_provided` | pass |
| works with async callback | `works_with_async_callback` | pass |
| works with async generator callback | `works_with_async_generator_callback` | pass |

## `tools/__tests__/zod-tool.test.ts` → `.typed_handler()` path

| TS test | Rust test | Status |
|---|---|---|
| creates tool with correct properties | `creates_tool_with_correct_properties` | pass |
| handles optional description | `handles_optional_description` | pass |
| invoke / handles synchronous callback | `typed_handles_synchronous_callback` | pass |
| invoke / handles promise callback | `typed_handles_promise_callback` | pass |
| invoke / handles async generator callback | `typed_handles_async_generator_callback` | pass |
| invoke / throws on invalid input | `typed_throws_on_invalid_input` | pass |
| invoke / validates required fields | `typed_validates_required_fields` | pass |
| invoke / passes context to callback | `typed_passes_context_to_callback` | pass |
| stream / streams synchronous callback result | `typed_streams_synchronous_callback_result` | pass |
| stream / streams promise callback result | `typed_streams_promise_callback_result` | pass |
| stream / streams async generator callback results | `typed_streams_async_generator_callback_results` | pass |
| stream / returns error result on validation failure | `typed_returns_error_result_on_validation_failure` | pass |
| stream / returns error result on missing required fields | `typed_returns_error_result_on_missing_required_fields` | pass |
| catches callback errors and returns error result | `typed_catches_callback_errors_and_returns_error_result` | pass |
| catches async callback errors | `typed_catches_async_callback_errors` | pass |
| handles nested object schemas | `typed_handles_nested_object_schemas` | pass |
| handles enum schemas | `typed_handles_enum_schemas` | pass |
| handles optional fields | `typed_handles_optional_fields` | pass |
| handles array schemas | `typed_handles_array_schemas` | pass |
| generates valid JSON schema from Zod schema | `generates_valid_json_schema_from_input_type` (schemars; asserts no `$schema`) | pass |
| passes instanceof Tool check and has InvokableTool methods | `typed_tool_is_usable_as_a_tool_ref` | pass |
| inputSchema undefined / default empty-object schema (5 tests) | covered by the untyped `optional inputSchema` group above | pass |
| inputSchema `z.void()` / default empty-object schema, invoke `{}`, stream, async generator (4 tests) | `typed_empty_input_creates_object_schema`, `typed_empty_input_invokes_with_empty_object`, `typed_empty_input_streams_with_empty_input`, `typed_empty_input_works_with_async_generator_callback` (empty struct input) | pass |
| `z.void()` does not throw Zod conversion errors | — | **skipped**: no Zod-conversion step; `schemars` cannot fail at build time. |
| `zod-tool.test-d.ts` (type-level) | — | not applicable (the Rust API is generic over the input type) |
| *(new)* explicit `.input_schema()` wins over the derived one | `typed_handler_honours_an_explicit_input_schema` | pass (added) |

## `tools/__tests__/tool-factory.test.ts`

| TS test | Rust test | Status |
|---|---|---|
| creates ZodTool when inputSchema is a Zod type | `creates_tool_with_correct_properties` (`.typed_handler`) | pass (dispatch is static in Rust: `handler` vs `typed_handler`) |
| creates FunctionTool when inputSchema is a plain object | `tool_factory_builds_an_untyped_tool` | pass |
| creates FunctionTool when inputSchema is omitted | `creates_tool_with_default_empty_object_schema` | pass |
| FunctionTool invoke() / synchronous, promise, async generator | `tool_factory_untyped_invoke_handles_values_and_progress`, `invoke_raw_returns_the_raw_value_and_propagates_errors` | pass |
| passes instanceof Tool | `maintains_stable_interface_signature` | pass |
| defaults description to the empty string | `tool_factory_defaults_description_to_the_empty_string` | pass |

## `tools/__tests__/noop-tool.test.ts` (TS has only the spec constant)

| TS artifact | Rust test | Status |
|---|---|---|
| `NOOP_TOOL_SPEC` name/description/inputSchema | `noop_tool.rs::tests::spec_matches_the_noop_tool_spec` | pass |
| *(result borrowed from `strands-py` `noop_tool`)* | `noop_tool.rs::tests::invoke_returns_the_ignore_instruction` | pass |

## `tools/__tests__/structured-output-tool.test.ts`

| TS test | Rust test | Status |
|---|---|---|
| builds tool spec from schema | `builds_tool_spec_from_schema` | pass |
| uses base description when schema has no description | `uses_base_description_when_schema_has_no_description` | pass |
| returns success with validated JSON for valid input | `returns_success_with_validated_json_for_valid_input` | pass |
| returns error with ZodError for invalid input | `returns_error_with_validation_message_for_invalid_input` | pass |
| includes validation details for multiple fields | `includes_validation_details_for_multiple_fields` | pass |
| validates nested objects | `validates_nested_objects` | pass |
| validates arrays | `validates_arrays` | pass |
| handles optional fields | `handles_optional_fields` | pass |
| returns error result for non-ZodError exceptions | `returns_error_result_for_an_uncompilable_schema` | pass (nearest analogue: `schema.parse` cannot be mocked; an uncompilable schema is the non-validation failure path) |
| *(new)* `::new::<T>()` derives the schema and the description | `derives_the_schema_from_a_rust_type` | pass (added) |

## Deviations from TS

1. **No `ToolStreamEvent` yielding.** `Tool::invoke` returns a `ToolResultBlock`; progress goes
   through `ctx.progress(value).await` (per `DESIGN.md`). Tests assert the drained progress channel
   where TS asserts yielded events.
2. **`_wrapInToolResult` step 1 (media-block instances) is unreachable** — a Rust handler yields
   `JsonValue`, so a media block arrives as `{"image": {..}}` and is handled by the single-key
   `_asToolResultContent` path. Behaviour is identical for every JSON-representable value.
3. **No `.typed::<I>()` step.** `I` is inferred from the `typed_handler` closure, so the extra
   turbofish call would be redundant; `.input_schema(..)` before `.typed_handler(..)` still wins.
4. **Validation messages are jsonschema's, not Zod's**: a pretty-printed JSON array of
   `{"path": "/age", "message": ".."}` — same shape and same "contains the field name" property
   that feeds the model's retry, different wording.
5. **No value transformation on validation.** `jsonschema` validates but does not strip unknown
   keys, so `StructuredOutputTool` returns the input verbatim where Zod would return the parsed
   object. The typed `FunctionTool` path does strip them, via `serde` deserialization into `I`.
6. **`ZodVoid` special case dropped**: a no-input tool is `.handler(..)` (ignores input) or
   `.typed_handler(|_: EmptyStruct, ..)`; there is no `()`-input schema conversion to guard against.
7. **Handler return type is `Result<R: Serialize>`**, not `Result<JsonValue>` — serialization
   happens inside the tool, which is also what makes the "unserializable value → error result"
   path (TS circular references) reachable.
8. `schemars` schemas keep their `title` (only `$schema` is stripped, mirroring
   `zodSchemaToJsonSchema`) and do not set `additionalProperties: false`, which matches Zod's
   *runtime* behaviour of ignoring unknown keys rather than its generated schema.

## Stubs / cross-module needs

- `#[cfg(test)] pub(crate) fn agent::test_agent()` + a `#[cfg(test)] MockModel` (empty stream) were
  appended to `src/agent/mod.rs` — the only file touched outside the module. If the agent module
  later ships a shared `test_support` fixture, this should fold into it.
- `#[cfg(test)] pub(crate) fn tools::tool::test_context(..)` added to `tools/tool.rs` (no existing
  signature changed) and used by the `function_tool`, `noop_tool` and `structured_output` tests.
