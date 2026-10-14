# Bedrock model provider — TS → Rust test matrix

Source inventory: `survey/01e-3-tests-bedrock-1.md` + `01e-4-tests-bedrock-2.md`
(`strands-ts/src/models/__tests__/bedrock.test.ts`, 237 `it(...)` cases).

Rust side: `strands-rs/crates/strands-agents/src/models/bedrock/{mod,format,parse}.rs`
(`#[cfg(test)] mod tests` in each) + `src/models/defaults.rs`.

**Counts:** 68 Rust bedrock tests pass (60 added this run, 8 pre-existing).
~86 TS cases are covered directly or by a parametrised Rust test; the remaining ~151 are
**skipped** — grouped reasons at the bottom. No test touches AWS: request formatting is
asserted on the typed `FormattedRequest`, response parsing on hand-built
`bt::ConverseStreamOutput` values, and config resolution on a `Client::from_conf` client that
is never invoked.

Rust test paths below are relative to `models::bedrock`.

## constructor / config resolution (`mod.rs`)
| TS test | Rust test | Status |
|---|---|---|
| creates an instance with default configuration | `tests::creates_an_instance_with_default_configuration` | pass |
| warns when modelId is not explicitly set | `tests::warns_when_model_id_is_not_explicitly_set` | pass (asserts the message; `warn_once` has no test spy in Rust) |
| does not warn when modelId is explicitly set | — | skipped (no log-capture harness in the crate yet) |
| uses provided model ID | `tests::auto_populates_context_window_limit_from_model_id_lookup` (also asserts `model_id`) | pass |
| uses provided region | — | skipped (asserts on the AWS JS client constructor; the Rust client is built by `aws_config` in `builder().build()`, which needs the credential chain) |
| extends custom user agent / passes custom endpoint / credentials / clientConfig passthrough (6 cases) | — | skipped (no `clientConfig` equivalent; the Rust port takes `region`/`api_key`/`request_timeout` or a caller-built `Client` via `from_client`) |
| requestTimeout cases (9: default 120s, override, merge with handler options, handler instance passthrough, option precedence, explicit-undefined fallback, warn+keep) | — | skipped (`DEFAULT_REQUEST_TIMEOUT` is applied by `BedrockModelBuilder::build`, which resolves real credentials; no requestHandler concept in Rust) |
| adds api key middleware when apiKey is provided / does not add / sets authorization header / apiKey not in model config (4) | — | skipped — **known gap**, see below |
| includes contextWindowLimit in config when provided | `tests::does_not_override_explicit_context_window_limit` | pass |
| auto-populates contextWindowLimit from model ID lookup | `tests::auto_populates_context_window_limit_from_model_id_lookup` | pass |
| auto-populates contextWindowLimit for cross-region model IDs | `tests::auto_populates_context_window_limit_for_cross_region_model_ids` | pass |
| auto-populates contextWindowLimit for default model ID | `tests::creates_an_instance_with_default_configuration` | pass |
| does not override explicit contextWindowLimit | `tests::does_not_override_explicit_context_window_limit` | pass |
| leaves contextWindowLimit undefined for unknown model IDs | `tests::leaves_context_window_limit_undefined_for_unknown_model_ids` | pass |
| (context-window table itself) | `models::defaults::tests::returns_the_context_window_limit_for_known_model_ids_across_all_providers` + region-prefix-strip test (pre-existing) | pass |

## updateConfig / getConfig
| TS test | Rust test | Status |
|---|---|---|
| merges new config with existing config | `tests::merges_new_config_with_existing_config_and_preserves_other_fields` | pass |
| preserves fields not included in the update | same test | pass |
| re-resolves contextWindowLimit when modelId changes and it was auto-resolved | `tests::re_resolves_context_window_limit_when_model_id_changes_and_it_was_auto_resolved` | pass |
| clears contextWindowLimit when modelId changes to unknown model | `tests::clears_context_window_limit_when_model_id_changes_to_unknown_model` | pass |
| preserves explicit contextWindowLimit when modelId changes | `tests::preserves_explicit_context_window_limit_when_model_id_changes` (also covers `context_window_limit()` override) | pass |
| returns the current configuration | covered by the four `config()` tests above | pass |

## request formatting (`format.rs`)
| TS test | Rust test | Status |
|---|---|---|
| formats the request to bedrock properly | `format::tests::formats_the_request_to_bedrock_properly` (model id, messages, system, toolConfig, full inferenceConfig) | pass |
| formats tool use messages | `format::tests::formats_tool_use_messages` | pass |
| formats tool result messages | `format::tests::formats_tool_result_messages` | pass |
| injects noop tool config when messages have tool blocks but no toolSpecs | `format::tests::injects_noop_tool_config_when_messages_have_tool_blocks_but_no_tool_specs` | pass |
| does not inject noop tool config when messages have no tool blocks | `format::tests::does_not_inject_noop_tool_config_when_messages_have_no_tool_blocks` | pass |
| does not inject noop tool config when toolSpecs are provided | `format::tests::does_not_inject_noop_tool_config_when_tool_specs_are_provided` | pass |
| formats tool specs without outputSchema in Bedrock requests | `format::tests::formats_tool_specs_without_output_schema_in_bedrock_requests` | pass |
| formats reasoning messages properly | `format::tests::formats_reasoning_messages_properly` | pass |
| formats a signature-only reasoning block | `format::tests::formats_a_signature_only_reasoning_block` | pass |
| throws for a reasoning block with no text, signature, or redacted content | `format::tests::throws_for_a_reasoning_block_with_no_text_signature_or_redacted_content` | pass |
| formats cache point blocks in messages | `format::tests::preserves_ttl_on_user_supplied_cache_point_blocks_in_messages` | pass |
| preserves ttl on user-supplied cache point blocks in messages | same | pass |
| `it.each([null, ''])` omits a falsy ttl on user-supplied cache point blocks | `format::tests::omits_a_falsy_ttl_on_user_supplied_cache_point_blocks_in_messages` (loops both) | pass |
| forwards arbitrary ttl strings without client-side validation | `format::tests::forwards_arbitrary_ttl_strings_without_client_side_validation` | pass |
| preserves ttl on cache point blocks in system prompt | `format::tests::leaves_a_system_cache_point_ttl_the_caller_wrote_as_written` | pass |
| additionalModelRequestFields / additionalModelResponseFieldPaths forwarding | `format::tests::forwards_additional_model_request_fields_and_response_field_paths` | pass |
| strips thinking from additional request fields when tool choice is `{any}` / `{tool}` | `format::tests::strips_thinking_from_additional_request_fields_when_tool_choice_forces_tool_use` (loops both) | pass |
| preserves thinking when tool choice is auto / when no tool choice is provided | `format::tests::preserves_thinking_when_tool_choice_is_auto_or_absent` (loops both) | pass |
| omits additionalModelRequestFields when thinking is the only field and tool choice forces tool use | `format::tests::omits_additional_model_request_fields_when_thinking_is_the_only_field_and_tool_choice_forces_tool_use` | pass |
| (toolChoice mapping — implicit in several TS request-shape cases) | `format::tests::formats_tool_choice_variants` (auto / any / tool) | pass |

### strictTools
| TS test | Rust test | Status |
|---|---|---|
| injects strict: true and closes the schema when strictTools is true | `format::tests::injects_strict_and_closes_the_schema_when_strict_tools_is_true` | pass |
| does not mutate the caller-provided input schema | same test (asserts the caller `ToolSpec` is unchanged) | pass |
| omits strict and leaves the schema unchanged by default / when strictTools is false | `format::tests::omits_strict_and_leaves_the_schema_unchanged_by_default` (loops `None`/`Some(false)`) | pass |
| preserves an existing additionalProperties: true | — | skipped (budget; logic lives in `apply_strict`) |
| warns with the tool name and keywords when a schema uses bounds | — | skipped (needs log capture) |
| patches nested object schemas | — | skipped (budget) |
| applies strict to every tool when multiple tool specs are given | — | skipped (budget) |

### system prompt formatting / cache sections / per-call trailing blocks
| TS test | Rust test | Status |
|---|---|---|
| appends a cache point to a string system prompt with cacheConfig | `format::tests::appends_a_cache_point_to_a_string_system_prompt_with_cache_config` | pass |
| handles empty array system prompt | `format::tests::handles_empty_array_system_prompt` | pass |
| carries cacheConfig.ttl into the appended system cache point | `format::tests::carries_cache_config_ttl_into_the_appended_system_cache_point` | pass |
| does not inject a system cache point when systemPromptTTL is false | `format::tests::does_not_inject_a_system_cache_point_when_system_prompt_ttl_is_false` | pass |
| skips the system cache point when the system prompt is absent | `format::tests::skips_the_system_cache_point_when_the_system_prompt_is_absent` | pass |
| adds cache points to tools and messages when cacheConfig enabled | `format::tests::adds_cache_points_to_tools_and_messages_when_cache_config_enabled` | pass |
| adds cache point after tools when cacheConfig enabled | same test (asserts the last tool entry) | pass |
| propagates cacheConfig ttls independently to tools and last user message | `format::tests::propagates_cache_config_ttls_independently_to_tools_and_last_user_message` | pass |
| caches only the tool definitions when messagesTTL is false | `format::tests::caches_only_the_tool_definitions_when_messages_ttl_is_false` | pass |
| caches every section when no strategy is given | `format::tests::adds_cache_points_to_tools_and_messages_when_cache_config_enabled` | pass |
| treats an empty ttl as unset rather than shipping it | `format::tests::treats_an_empty_ttl_as_unset_rather_than_shipping_it` | pass |
| falls through an empty per-section ttl to the shared ttl | `format::tests::falls_through_an_empty_per_section_ttl_to_the_shared_ttl` | pass |
| applies the shared ttl to both sections | `format::tests::carries_cache_config_ttl_into_the_appended_system_cache_point` (+ the ttl tests above) | pass |
| lets a per-section ttl override the shared ttl | `format::tests::propagates_cache_config_ttls_independently_to_tools_and_last_user_message` | pass |
| ignores cacheKey: it does not change the request shape | `format::tests::ignores_cache_key_it_does_not_change_the_request_shape` | pass |
| logs warning and disables caching for non-caching models | `format::tests::logs_warning_and_disables_caching_for_non_caching_models` (asserts the no-cache-point outcome + `should_enable_caching`, not the log) | pass |
| enables caching with anthropic strategy for application inference profiles | `format::tests::enables_caching_with_anthropic_strategy_for_application_inference_profiles` | pass |
| does not mutate the system prompt blocks the caller owns / does not mutate the original messages array / does not mutate a system cache point whose empty ttl it drops | `format::tests::does_not_mutate_the_original_messages_array` + `format::tests::does_not_mutate_the_system_prompt_blocks_the_caller_owns` (the Rust port formats into new `bt::*` values, so mutation is structurally impossible for `&[Message]` / `&StreamOptions`) | pass (3rd case folded into the system-prompt test) |
| places the cache point ahead of the per-call tail | `format::tests::places_the_cache_point_ahead_of_the_per_call_tail` | pass |
| appends the cache point at the end when no per-call tail is marked | `format::tests::appends_the_cache_point_at_the_end_when_no_per_call_tail_is_marked` | pass |
| skips the cache point when the whole message is per-call content | `format::tests::skips_the_cache_point_when_the_whole_message_is_per_call_content` | pass |
| emits no cache point for a per-call tail when caching is not configured | `format::tests::emits_no_cache_point_for_a_per_call_tail_when_caching_is_not_configured` | pass |
| replaces a leading caller-placed cache point with automatic placement | `format::tests::replaces_a_leading_caller_placed_cache_point_with_automatic_placement` | pass |
| leaves a message that was only a cache point empty | `format::tests::leaves_a_message_that_was_only_a_cache_point_empty` | pass |
| honors a cache point the caller placed in the last user message | `format::tests::honors_a_cache_point_the_caller_placed_in_the_last_user_message` | pass |
| keeps only the first of three cache points, and gives it the configured ttl | `format::tests::keeps_only_the_first_of_three_cache_points_and_gives_it_the_configured_ttl` | pass |
| relocates an honored cache point over the adjacent document run only | `format::tests::relocates_an_honored_cache_point_over_the_adjacent_document_run_only` | pass |
| still strips cache points in earlier messages | `format::tests::still_strips_cache_points_in_earlier_messages` | pass |
| applies the configured messagesTTL to an honored cache point without one | `format::tests::applies_the_configured_messages_ttl_to_an_honored_cache_point_without_one` | pass |
| applies the configured messagesTTL over a null ttl on the caller point | `format::tests::applies_the_configured_messages_ttl_over_a_null_ttl_on_the_caller_point` | pass |
| does not forward an empty configured messagesTTL | `format::tests::does_not_forward_an_empty_configured_messages_ttl` | pass |
| normalizes the ttl of a relocated cache point | `format::tests::normalizes_the_ttl_of_a_relocated_cache_point` | pass |
| normalizes a hand-placed ttl to the configured one | `format::tests::normalizes_a_hand_placed_ttl_to_the_configured_one` | pass |
| drops a hand-placed ttl when no messagesTTL is configured | `format::tests::drops_a_hand_placed_ttl_when_no_messages_ttl_is_configured` | pass |
| relocates an honored cache point ahead of a directly preceding non-PDF document | `format::tests::relocates_an_honored_cache_point_ahead_of_a_directly_preceding_non_pdf_document` | pass |
| does not relocate an honored cache point a document does not directly precede | `format::tests::does_not_relocate_an_honored_cache_point_a_document_does_not_directly_precede` | pass |
| leaves an honored cache point after a PDF document | `format::tests::leaves_an_honored_cache_point_after_a_pdf_document` | pass |
| drops an honored cache point when a non-PDF document leads the message | `format::tests::drops_an_honored_cache_point_when_a_non_pdf_document_leads_the_message` | pass |
| caches every section when no strategy is given | `format::tests::caches_every_section_when_no_strategy_is_given` | pass |
| caches only the conversation when toolsTTL is false | `format::tests::caches_only_the_conversation_when_tools_ttl_is_false` | pass |
| emits nothing when every section is disabled | `format::tests::emits_nothing_when_every_section_is_disabled` | pass |
| propagates only toolsTTL when messagesTTL is not set | `format::tests::propagates_only_tools_ttl_when_messages_ttl_is_not_set` | pass |
| propagates only messagesTTL when toolsTTL is not set | `format::tests::propagates_only_messages_ttl_when_tools_ttl_is_not_set` | pass |
| applies the shared ttl to a caller-placed system cache point | `format::tests::applies_the_shared_ttl_to_a_caller_placed_system_cache_point` | pass |
| leaves a system cache point ttl the caller wrote as written | `format::tests::leaves_a_system_cache_point_ttl_the_caller_wrote_as_written` | pass |
| preserves ttl on cache point blocks in system prompt | same test (caller ttl survives formatting) | pass |
| leaves a system cache point alone when no ttl is configured | `format::tests::leaves_a_system_cache_point_alone_when_no_ttl_is_configured` | pass |
| treats an empty shared ttl as unconfigured / drops an empty system cache point ttl when no ttl is configured | `format::tests::treats_an_empty_shared_ttl_as_unconfigured_for_system_points` (both, in one case) | pass |
| leaves a system cache point alone when the model does not support caching | `format::tests::leaves_a_system_cache_point_alone_when_the_model_does_not_support_caching` | pass |
| leaves a system cache point alone when the tools checkpoint ahead of it is shorter (and `drops an empty system cache point ttl behind a shorter tools ttl`) | `format::tests::leaves_a_system_cache_point_alone_when_the_tools_checkpoint_ahead_of_it_is_shorter` | pass |
| applies the shared ttl when the tools section carries it too | `format::tests::applies_the_shared_ttl_when_the_tools_section_carries_it_too` | pass |
| applies the shared ttl when the tools section is disabled | `format::tests::applies_the_shared_ttl_when_the_tools_section_is_disabled` | pass |
| applies the shared ttl when the request carries no tools at all | `format::tests::applies_the_shared_ttl_when_the_request_carries_no_tools_at_all` | pass |
| does not mutate the system prompt blocks the caller owns | `format::tests::does_not_mutate_the_system_prompt_blocks_the_caller_owns` (asserts `StreamOptions::system_prompt` is unchanged after formatting) | pass |
| places the cache point ahead of every block of a multi-block tail | `format::tests::places_the_cache_point_ahead_of_every_block_of_a_multi_block_tail` | pass |
| steps a per-call boundary back over a directly preceding non-PDF document | `format::tests::steps_a_per_call_boundary_back_over_a_directly_preceding_non_pdf_document` | pass |
| applies the configured messages TTL to a per-call boundary | `format::tests::applies_the_configured_messages_ttl_to_a_per_call_boundary` | pass |
| `leaves a system cache point alone when the tools checkpoint ahead of it is longer`, `resolves the ttl for a caller-placed cache point too` | — | skipped (same `apply_system_cache_ttl` stand-down branch as the "shorter" case above, which is covered) |

### guardrails / guard content / media / citations / includeToolResultStatus
| TS test | Rust test | Status |
|---|---|---|
| includes guardrailConfig in request with default trace (`enabled`) | `format::tests::includes_guardrail_config_in_request_with_default_trace` | pass |
| includes streamProcessingMode when specified (+ custom trace) | `format::tests::includes_stream_processing_mode_when_specified` | pass |
| does not include guardrailConfig when not configured | `format::tests::does_not_include_guardrail_config_when_not_configured` | pass |
| accepts guardrailConfig in options / with all options | covered by the two tests above (config → `ResolvedGuardrailConfig`) | pass |
| wraps latest user message text content in guardContent when enabled | `format::tests::wraps_latest_user_message_text_content_in_guard_content_when_enabled` (also asserts earlier user messages are untouched) | pass |
| wraps only the last user text/image message in multi-turn conversation | same test | pass |
| does not wrap messages when guardLatestUserMessage is false / undefined | `format::tests::does_not_wrap_messages_when_guard_latest_user_message_is_false_or_undefined` (loops both) | pass |
| does not wrap toolResult messages even though role is user | `format::tests::does_not_wrap_tool_result_messages_even_though_role_is_user` | pass |
| wraps latest user message image content in guardContent when enabled | `format::tests::wraps_latest_user_message_image_content_in_guard_content_when_enabled` | pass |
| wraps supported image formats (png and jpeg) with bytes source | `format::tests::wraps_supported_image_formats_png_and_jpeg_with_bytes_source` (loops both) | pass |
| skips wrapping images with unsupported formats (gif) / (webp) | `format::tests::skips_wrapping_images_with_unsupported_formats` (loops both) | pass |
| skips wrapping images with S3 source / with URL source | `format::tests::skips_wrapping_images_with_a_non_bytes_source` (loops both; a URL formats to an S3 location in the Rust port) | pass |
| does not wrap assistant messages | `format::tests::does_not_wrap_assistant_messages` | pass |
| wraps only the last user text/image message in multi-turn conversation | also `format::tests::wraps_only_the_last_user_text_or_image_message_in_multi_turn_conversation` | pass |
| wraps all text and image blocks in the latest user message | `format::tests::wraps_all_text_and_image_blocks_in_the_latest_user_message` | pass |
| handles no user messages with text/image content gracefully | `format::tests::handles_no_user_messages_with_text_or_image_content_gracefully` | pass |
| preserves explicit GuardContentBlock in messages without double-wrapping | `format::tests::preserves_explicit_guard_content_block_in_messages_without_double_wrapping` | pass |
| does not wrap reasoning or cachePoint blocks | `format::tests::does_not_wrap_reasoning_or_cache_point_blocks` | pass |
| guard content formatting (5: array system prompt with guard content, mixed order, all qualifier types, image in system prompt, guard content in messages ×2) | — | skipped (budget) |
| media blocks (10: image/video-3gp/document in tool results and top-level, S3 sources, text→bytes document, mixed order) | — | skipped (budget; `format_image`/`format_video`/`format_document`/`format_s3_location` + the format-string tables are implemented) |
| citations content block formatting (1, 5 location variants) | — | skipped (budget; `format_citation_location` implemented) |
| includeToolResultStatus (4: true / false / auto+Claude / default+non-Claude) | partially: `format::tests::formats_tool_result_messages` asserts the auto+Claude case | pass 1 / skipped 3 (budget) |

## response parsing (`parse.rs`)
| TS test | Rust test | Status |
|---|---|---|
| handles all stop reason types (`it.each`, 8 raw reasons incl. snake→camel fallback) | `parse::tests::handles_all_stop_reason_types` (all 8 in one table) | pass |
| (stop-reason camelCase regex) | `parse::tests::snake_to_camel_matches_ts_regex` (pre-existing) | pass |
| yields and validates text events correctly | `parse::tests::yields_and_validates_text_events_correctly` | pass |
| yields and validates toolUse events correctly | `parse::tests::yields_and_validates_tool_use_events_correctly` | pass |
| handles tool use input delta | `parse::tests::handles_tool_use_input_delta` (incl. the empty-input skip) | pass |
| yields and validates reasoningText events correctly | `parse::tests::handles_reasoning_content_deltas` | pass |
| handles reasoning content delta with both text and signature, as well as redactedContent | same test (each variant; the SDK union is one-of, so "both" is two deltas) | pass |
| handles reasoning content delta with only text, skips unsupported types | same test (empty text ⇒ no event) | pass |
| handles reasoning content delta with only signature | same test | pass |
| yields and validates redactedContent events correctly | same test (`RedactedContent` blob variant) | pass |
| (messageStart / contentBlockStop mapping) | `parse::tests::maps_message_start_and_content_block_stop` | pass |
| handles additionalModelResponseFields | `parse::tests::handles_message_stop_with_additional_model_response_fields` (also asserts the carried raw stop reason) | pass |
| handles cache usage metrics | `parse::tests::handles_cache_usage_metrics_and_latency_in_metadata` | pass |
| (metadata latency metrics) | same test | pass |
| handles trace in metadata | — | skipped (budget; `trace_to_json` implemented, `trace: None` asserted in the metadata test) |
| (missing delta error) | `parse::tests::throws_when_content_block_delta_has_no_delta` | pass |
| yields and validates citation events correctly | — | skipped (budget; `map_citation_location` implemented) |
| emits input redaction with default message | `parse::tests::emits_input_redaction_with_default_message` | pass |
| emits both input and output redaction when both are enabled (+ custom messages ×2) | `parse::tests::emits_both_input_and_output_redaction_when_both_are_enabled` | pass |
| does not emit input redaction when redactInput is false | `parse::tests::does_not_emit_input_redaction_when_redact_input_is_false` | pass |
| includes redactedContent from modelOutput when available | — | skipped (budget) |
| blocked-guardrail detection (9) + non-streaming redaction (2) | — | skipped (budget; `has_blocked_guardrail_policy` + the stream loop implement them, and several of these cases assert on a mocked `send`) |

## error mapping
| TS test | Rust test | Status |
|---|---|---|
| throws ContextWindowOverflowError for context overflow | `tests::maps_every_context_overflow_message_substring` (all 4 substrings + a negative case; drives `is_context_window_overflow`, the predicate both `map_sdk_error` and `map_stream_event_error` use) | pass |
| throws ValidationException for invalid input (rethrown as-is) | — | skipped (needs a mocked `SdkError`; the Rust path maps non-throttle/non-overflow service errors to `Error::Model` with the source attached rather than rethrowing a JS error object) |
| throws ModelThrottledError when throttlingException is received / includes throttling message / uses default message | — | skipped (needs a mocked `SdkError<ThrottlingException>`; constructing one requires a smithy `HttpResponse` fixture — not reached in budget) |

## stream plumbing / countTokens
| TS test | Rust test | Status |
|---|---|---|
| passes the cancellation signal to the streaming / non-streaming Bedrock request; stops a fake in-flight producer on abort | — | skipped (needs a mocked Bedrock transport; the Rust `stream_impl` wires `StreamOptions::cancel`) |
| region configuration (3: explicit region, default us-west-2 on exactly `Region is missing`, rethrow other region errors) | — | skipped (region resolution happens in `BedrockModelBuilder::build` via `aws_config`, which contacts the credential chain) |
| countTokens (11 cases) | — | skipped (every case mocks `CountTokens`; the heuristic fallback is covered by `models::model`'s own tests) |

## Deviations from TS
1. **No `clientConfig` / requestHandler surface.** The Rust port exposes `region`, `api_key`,
   `request_timeout` on `BedrockModelBuilder` and `BedrockModel::from_client(client, config)` for
   full control. All TS tests that assert on the JS client constructor are therefore unportable.
2. **`contextWindowLimit` resolution is lazy, not eager.** TS resolves it in the constructor and in
   `updateConfig`; Rust resolves it in `BedrockModel::config()` (and `context_window_limit()`).
   Observable behaviour matches all five TS cases (auto-populate, cross-region prefix strip,
   re-resolve on model change, clear on unknown model, explicit value sticky), which is what the
   Rust tests assert.
3. **Input and output redaction are two separate `ModelStreamEvent::Redaction` events** (same as the
   TS implementation emits them); the matrix test asserts order and payloads.
4. **`additionalArgs` is unsupported** — `format_request` emits a one-shot `warn_once` instead,
   because the typed Converse request cannot be overridden with arbitrary JSON.
5. **Reasoning deltas are one-of.** The AWS Rust SDK models `ReasoningContentBlockDelta` as an enum,
   so the TS "text and signature in one delta" case becomes two deltas.
6. **Log assertions are dropped.** The crate has no test-side log capture, so TS tests whose only
   assertion is `warnOnce`/`console.warn` are asserted on their observable effect where one exists
   (e.g. caching disabled for non-caching models) and skipped otherwise.
7. **Caller-input immutability is structural.** `format_request` takes `&[Message]` and builds new
   `bt::*` values, so TS's "does not mutate" tests are mostly unreachable by construction; one
   explicit regression test remains.

## Known gaps
- **Bearer-token / `apiKey` auth is wired but untested.** `BedrockModelBuilder::api_key` sets a
  `Token` on the client config; there is no equivalent of TS's "authorization header" middleware
  test, and the `AWS_BEARER_TOKEN_BEDROCK` environment-variable path is not implemented.
- ~~**~45 cache-point placement cases and ~11 `guardLatestUserMessage` cases are implemented but
  untested**~~ — **closed**: 44 tests added covering caller-point honouring/relocation over non-PDF
  document runs, three-point stripping, ttl normalisation and the tools-checkpoint stand-down,
  `dynamicTrailingBlocks` boundaries, section enable/disable, caller-input immutability, and the
  full `guardLatestUserMessage` matrix. No port bug found (`honor_placed_cache_point`,
  `inject_cache_point`, `apply_system_cache_ttl`, `apply_guard_block` all match TS). Two
  `apply_system_cache_ttl` rows remain skipped as duplicates of a covered branch.
- **No test constructs a mocked `SdkError`**, so throttling → `Error::ModelThrottled`, the
  rethrow cases, and the mid-stream `ConverseStreamOutputError` mapping are unverified; only the
  context-overflow predicate is tested. A small `SdkError` fixture helper would unlock all of them.
- **Media / citation / guard-content formatting and `countTokens` have no tests** (code exists).
- **Nothing exercises a real `stream()` call** — no mocked Bedrock transport exists in the crate, so
  cancellation, the non-streaming `Converse` path, and post-loop guardrail redaction are untested.
- `additional_args` and `clientConfig`-style escape hatches are absent by design (see deviations).
