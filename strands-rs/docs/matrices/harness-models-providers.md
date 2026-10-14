# harness-rs models.rs — anthropic / openai / bedrock-mantle provider arms

`resolve_model` now dispatches per provider (`build_bedrock`/`build_anthropic`/`build_openai`,
each with a `#[cfg(feature = ...)]` arm + a "needs the `<feat>` feature" fallback that still runs
`resolve_spec` first so validation errors match). `google`/`litellm`/`ollama` keep the
unsupported-provider error (message now lists the built providers). New `pub(crate)`
`anthropic_config_for` / `openai_config_for` mirror `bedrock_config_for` (pure, no network).
Harness features: added `anthropic`, `openai` (both in `default`).

| TS test (test/models.test.ts) | Rust test | status |
|---|---|---|
| 236 maps openai to reasoning effort | `openai_tests::maps_openai_to_reasoning_effort` | pass |
| 243 maps anthropic to thinking and max tokens | `anthropic_tests::maps_anthropic_to_thinking_and_max_tokens` | pass |
| 252 gives anthropic haiku its own tier max tokens | `anthropic_tests::gives_anthropic_haiku_its_own_tier_max_tokens` | pass |
| 257 flat max tokens for unrecognized anthropic tier | `anthropic_tests::falls_back_to_a_flat_max_tokens_for_an_unrecognized_anthropic_tier` | pass |
| 369 extended thinking params, anthropic-direct haiku | `anthropic_tests::sends_extended_thinking_params_to_an_anthropic_direct_haiku_model` | pass |
| 374 no thinking params below the floor | `anthropic_tests::sends_no_thinking_params_to_an_anthropic_direct_claude_below_the_floor` | pass |
| (363 analogue, anthropic-direct) | `anthropic_tests::rejects_a_thinking_level_on_an_anthropic_direct_claude_below_the_floor` | pass (added) |
| 399 builds bedrock-mantle via the OpenAI provider | `openai_tests::builds_bedrock_mantle_via_the_openai_provider` | pass |
| 408 rejects unsupported thinking level for bedrock-mantle | `openai_tests::rejects_an_unsupported_thinking_level_for_bedrock_mantle` | pass |
| 412 Bedrock Web Search tool for mantle web_search | `openai_tests::adds_the_bedrock_web_search_tool_for_bedrock_mantle_web_search` | pass |
| 419 + 427 Bedrock Web Search with effort off (luna, gpt-6-astra) | `openai_tests::adds_the_bedrock_web_search_tool_with_effort_off` (both ids) | pass |
| 435 omits mantle tools when web_search off | `openai_tests::omits_bedrock_mantle_tools_when_web_search_is_off` | pass |
| 441 openai web_search tool alongside reasoning | `openai_tests::adds_the_openai_web_search_tool_alongside_reasoning` | pass |
| 448 openai web_search tool with effort off | `openai_tests::adds_the_openai_web_search_tool_with_effort_off` | pass |
| 455 omits openai tools when web_search off | `openai_tests::omits_openai_tools_when_web_search_is_off` | pass |
| 471 anthropic web_search server tool as direct call | `anthropic_tests::adds_the_anthropic_web_search_server_tool_as_a_direct_call` | pass |
| 479 omits anthropic server tools when web_search off | `anthropic_tests::omits_anthropic_server_tools_when_web_search_is_off` | pass |
| 484 never adds native search where model has none | `openai_tests::never_adds_a_native_search_tool_where_the_model_has_none` | pass |
| 507 enables prompt caching for anthropic direct | `anthropic_tests::enables_prompt_caching_for_anthropic_direct_when_requested` | pass |
| 512 no cacheConfig when caching off | `anthropic_tests::does_not_set_cache_config_for_anthropic_direct_when_caching_is_off` | pass |
| (no TS row) mantle base URL/base path from model id | `openai_tests::derives_the_mantle_base_url_from_the_resolved_model_id` | pass (added) |

Deviations: TS `new OpenAIModel({ modelId })` when there are no params → Rust always passes a
`params` map (empty when no effort/tools), same request shape. Mantle region is left `None` in
`BedrockMantleConfig` (TS passes `{}`); the SDK resolves `AWS_REGION`/`AWS_DEFAULT_REGION` itself,
so the base-URL test supplies a region explicitly instead of mutating process env.
Counts: 21 rows, 21 pass, 0 skipped.

Gates (last line each):
- `cargo fmt` → (no output)
- `cargo clippy --all-targets --all-features -p strands-harness -- -D warnings` → `Finished \`dev\` profile [unoptimized] target(s) in 9.21s`
- `cargo test --all-features -p strands-harness` → `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` (209 lib tests ok; 7 live-Bedrock ignored)
