# anthropic (Pass B)
STATUS: merge of agent-tasks/strands-rs resolved (Cargo.toml features union; anthropic SSE parser
deleted in favour of shared `models::sse::SseParser`; `ReqwestTransport` re-exported at `models::`
as `AnthropicReqwestTransport` / `OpenAIReqwestTransport` to avoid the clash).
`cargo build --all-targets --all-features -p strands-agents` clean.
NEXT: run gates (fmt/clippy/test, --no-default-features), verify spec coverage vs p1/p3a.
