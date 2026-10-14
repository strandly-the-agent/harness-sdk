# harness-rs: builtin tools (`read`/`write`/`edit`, `web_fetch`, `web_search`)

Branch `agent-tasks/harness-rs-h-tools`, worktree `/home/sandbox/wt/h-tools`.

## Files
- `crates/strands-harness/src/tools/mod.rs` (+ `cfg(test)` `test_support`: `invoke`, `text_of`, `test_agent`)
- `crates/strands-harness/src/tools/file_tools.rs` — `read()`, `make_read(ReadConfig{media})`, `write()`, `edit()`
- `crates/strands-harness/src/tools/web_fetch.rs` — `make_web_fetch(MakeWebFetchOptions)`, `WebFetchTransport`, `FetchedPage`, `ReqwestTransport`
- `crates/strands-harness/src/tools/web_search.rs` — `search_tool`, `SearchBackend`, `ExaBackend`, `ExaTransport`, `ReqwestExaTransport`, `make_exa_web_search`, `exa_web_search`, `SearchResult`, `WebSearchError`
- `Cargo.toml` (workspace) + `crates/strands-harness/Cargo.toml`: `reqwest 0.12` (`default-features = false`, `rustls-tls`, `json`), optional behind feature `http` (in `default`)
- `src/lib.rs` re-exports

## Verbatim-from-TS surfaces
tool names/descriptions/schemas; `cat -n` numbering (`{:>6}\t`); `[Showing lines a-b of n. ...]`;
`[File has n lines; offset X is past the end.]`; media placeholders; `Wrote N lines to P.`;
`Edited P.`; `old_str did not appear verbatim in P.` / `old_str appears N times in P; make it unique.`;
path-validation errors; `web_fetch` description + SUMMARIZER_PROMPT + `Failed to fetch URL: msg` +
URL-validation errors + 15-min cache; `web_search` `Sources:` formatting, `No results.`,
`web_search failed: reason`, keyless rate-limit suffix, Exa `---`/`Title:`/`URL:`/`Highlights:` parsing,
500-char snippet, `max_results` clamp `max(1, min(n ?? 5, 10))`.
`html_to_text` is a hand-rolled port of the four TS regexes (no `regex` dep — TS adds no dep either);
output verified character-for-character against the TS function run under node for three inputs,
including the surprising `"a \n\n b"` spacing.

## Deviations (and why)
1. **No `Sandbox` seam in the Rust SDK.** TS routes file access through `context.agent.sandbox`;
   these tools use `tokio::fs` on the host. Same for `web_fetch`: TS's default `transport: 'curl'`
   (curl inside the sandbox) is **not ported**; the seam is the `WebFetchTransport` trait and the
   default `ReqwestTransport` is the equivalent of TS's `transport: 'direct'`. `MAX_BYTES` (5 MiB)
   is applied by truncating the buffered body rather than cancelling the stream mid-read.
2. **Charset decoding**: TS uses `TextDecoder(charset)`; Rust uses `String::from_utf8_lossy`
   (no encoding crate added). Non-UTF-8 pages degrade instead of being transcoded.
3. **No MCP client in the Rust SDK yet.** `ExaBackend` speaks the same MCP `tools/call` JSON-RPC to
   `https://mcp.exa.ai/mcp` directly over an injectable `ExaTransport` (default `reqwest`, handles an
   SSE `data:` framing). Model-facing behaviour, env var (`EXA_API_KEY`, read per call) and error
   strings are unchanged; the `listTools`/`disconnect` handshake (and its
   `Exa's MCP server does not offer web_search_exa` error) has no analogue and is not ported.
4. `read` returns media by serializing `ToolResultContent::{Image,Document}`, which
   `wrap_in_tool_result` recognises — the SDK's equivalent of returning an `ImageBlock` in TS.
5. `web_fetch`/`ExaBackend` default transports are behind feature `http` (default-on) so the crate
   still builds without `reqwest`.
6. Not ported (documented in `tools/mod.rs`): `programmatic-tool-caller` (Node-only), `subagent`
   (separate task).

## Test matrix (TS → Rust)
44 Rust tests in `strands-harness` (all passing); mapping below.

| TS test (`harness-ts/test/tools/...`) | Rust test | Status |
| --- | --- | --- |
| file-tools: numbered lines | `file_tools::read_returns_numbered_lines` | pass |
| file-tools: offset/limit paging + footer | `read_pages_with_offset_and_limit` | pass |
| file-tools: offset past end | `read_reports_an_offset_past_the_end` | pass |
| file-tools: image → ImageBlock | `read_returns_an_image_as_media` | pass |
| file-tools: pdf → DocumentBlock + sanitized name | `read_returns_a_document_as_media_with_a_sanitized_name` | pass |
| file-tools: `media: false` placeholder | `read_describes_media_when_media_is_off` | pass |
| file-tools: csv/txt stay text | `read_keeps_text_documents_on_the_numbered_path` | pass |
| file-tools: relative path rejected | `read_rejects_a_relative_path`, `write_rejects_a_relative_path` | pass |
| file-tools: traversal rejected | `read_rejects_path_traversal` | pass |
| file-tools: write + line count | `write_writes_and_counts_lines` | pass |
| file-tools: write empty = 0 lines | `write_counts_empty_content_as_zero_lines` | pass |
| file-tools: edit unique replace | `edit_replaces_a_unique_occurrence` | pass |
| file-tools: edit missing old_str | `edit_fails_when_old_str_is_absent` | pass |
| file-tools: edit ambiguous old_str | `edit_fails_when_old_str_is_ambiguous` | pass |
| file-tools: tool specs | `file_tools::specs_match_typescript` | pass |
| web-fetch: no prompt → raw content | `web_fetch::empty_prompt_returns_the_raw_content`, `blank_prompt_returns_the_raw_content` | pass |
| web-fetch: HTML reduced to text | `html_is_reduced_to_text`, `html_to_text_collapses_blank_lines` | pass |
| web-fetch: prompt → summarizer answer | `a_prompt_is_answered_by_the_summarizer` | pass |
| web-fetch: cache hit on second call | `content_is_cached_across_calls` | pass |
| web-fetch: fetch failure string | `a_fetch_failure_is_reported_to_the_model` | pass |
| web-fetch: non-http scheme | `rejects_a_non_http_scheme` | pass |
| web-fetch: no host | `rejects_a_url_with_no_host` | pass |
| web-fetch: disallowed characters | `rejects_disallowed_url_characters` | pass |
| web-fetch: summarizer prompt / tool spec | `summarizer_prompt_is_verbatim`, `web_fetch::spec_matches_typescript` | pass |
| web-fetch: curl command construction, `transport` validation, 30s timeout message, streaming byte cap | — | skipped (no sandbox seam; curl path not ported — see Deviation 1) |
| web-fetch: TextDecoder charset | — | skipped (Deviation 2) |
| web-search: `Sources:` formatting | `web_search::formats_results_as_numbered_sources` | pass |
| web-search: no results | `reports_no_results` | pass |
| web-search: backend failure string | `reports_a_backend_failure` | pass |
| web-search: default 5 results | `max_results_defaults_to_five` | pass |
| web-search: clamp to 1..10 | `max_results_is_clamped_to_one_through_ten` | pass |
| web-search: parse Exa blocks | `parses_exa_blocks` | pass |
| web-search: Title:/URL: inside highlights ignored | `ignores_a_block_without_a_title_url_head` | pass |
| web-search: 500-char snippet cap | `truncates_a_long_snippet` | pass |
| web-search: Exa call args + api key header | `exa_backend_sends_the_mcp_tool_call_and_parses_the_reply` | pass |
| web-search: `isError` surfaced | `exa_backend_surfaces_a_tool_error` | pass |
| web-search: keyless rate-limit suffix | `a_keyless_failure_mentions_the_rate_limit` | pass (self-skips if `EXA_API_KEY` is set) |
| web-search: tool spec | `web_search::spec_matches_typescript` | pass |
| web-search: `listTools` missing-tool error, client disconnect | — | skipped (no MCP client — Deviation 3) |

## Gates (from `strands-rs/`)
- `cargo fmt --check` → `fmt clean` (no output)
- `cargo clippy --all-targets --all-features -- -D warnings` → `    Finished \`dev\` profile [unoptimized] target(s) in 7.70s`
- `cargo test --all-features -p strands-harness` → `test result: ok. 44 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.09s`
- `cargo test --all-features` (workspace) → `test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s` (last of 12 suites; 662+33+24+3+10+44+1+1 passed, 0 failed)
