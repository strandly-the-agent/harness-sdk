| Model strings | `google/`, `ollama/` and `litellm/` build Gemini and OpenAI-compatible models | `model provider "google" is not supported in Rust yet` (also `ollama`, `litellm`) | Rust v0.1 has Bedrock, Bedrock Mantle, Anthropic and OpenAI (#60) |
| Unknown model provider | `Supported providers: anthropic, bedrock, bedrock-mantle, google, litellm, ollama, openai` | `Supported providers: anthropic, bedrock, bedrock-mantle, openai` | the list names what Rust builds (P-831) |
| Disabled provider | every provider is importable | `model '<spec>' requires the strands feature "<feature>"` when the provider's cargo feature is off | providers are cargo features in Rust |
| `supports_thinking`, `supports_web_search` on `google/...` | `true` | `false` | Google is not a Rust v0.1 provider |
| `web_fetch` summarizer for `ollama/`, `litellm/` | warns and reuses the main model | the unsupported-provider error | the main model itself can't be built |
| `supportsMedia` on a model instance | `false` only for a `BedrockModel` instance with an `openai.` family id | `false` for any instance whose model id has an `openai.` family | `Model` has no downcast (P-832) |
| `effort` outside the enum | `resolveModel` throws `Effort "x" is not valid. ...` | `Effort` is an enum; the same text comes from parsing a name (`parse_effort`) | typed API (P-833) |
