//! Ported from strands-ts/src/vended-tools/web-fetch/__tests__/web-fetch.test.ts
//!
//! Requests go to a local HTTP server instead of a mocked `fetch`, and the real converter runs
//! instead of a mocked `htmlToMarkdown`. The analyst runs on mock models.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic, reason = "test code")]

use std::num::NonZeroU32;
use std::sync::{Arc, Mutex};

use serde_json::json;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use super::*;
use crate::Tool;
use crate::models::model::Model;
use crate::models::streaming::Usage;
use crate::testing::{MockMessageModel, MockTurnOptions};
use crate::types::agent::{InvokeLimits, LocalAgent};
use crate::types::messages::{ReasoningBlock, Role, TextBlock};
use crate::vended_tools::test_support::{cancellable_context, context, context_for};

/// One scripted response: the head (status line and headers) and body bytes. `hang` keeps the
/// connection open after the body instead of finishing it.
#[derive(Clone)]
struct Reply {
    status: &'static str,
    content_type: Option<&'static str>,
    body: Vec<u8>,
    hang: bool,
}

fn reply(body: &str, content_type: &'static str) -> Reply {
    Reply {
        status: "200 OK",
        content_type: Some(content_type),
        body: body.as_bytes().to_vec(),
        hang: false,
    }
}

/// A local server answering every request with `reply`; returns its base URL and the raw
/// requests it received.
async fn server(reply: Reply) -> (String, Arc<Mutex<Vec<String>>>) {
    let (url, requests, _) = notifying_server(reply).await;
    (url, requests)
}

/// [`server`], also returning a notification sent once each response is written.
async fn notifying_server(reply: Reply) -> (String, Arc<Mutex<Vec<String>>>, Arc<Notify>) {
    let written = Arc::new(Notify::new());
    let notify = Arc::clone(&written);
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/", listener.local_addr().unwrap());
    let requests = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&requests);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                return;
            };
            let mut buf = vec![0u8; 8192];
            let n = socket.read(&mut buf).await.unwrap_or(0);
            seen.lock()
                .unwrap()
                .push(String::from_utf8_lossy(&buf[..n]).into_owned());
            let length = if reply.hang {
                reply.body.len() + 1
            } else {
                reply.body.len()
            };
            let mut head = format!("HTTP/1.1 {}\r\ncontent-length: {length}\r\n", reply.status);
            if let Some(content_type) = reply.content_type {
                head.push_str(&format!("content-type: {content_type}\r\n"));
            }
            head.push_str("\r\n");
            let _ = socket.write_all(head.as_bytes()).await;
            let _ = socket.write_all(&reply.body).await;
            let _ = socket.flush().await;
            notify.notify_one();
            if reply.hang {
                std::mem::forget(socket);
            }
        }
    });
    (url, requests, written)
}

fn markdown() -> FunctionTool {
    make_web_fetch(
        MakeWebFetchOptions::builder()
            .mode(WebFetchMode::Markdown)
            .build(),
    )
    .unwrap()
}

fn markdown_with(max_content_chars: usize) -> FunctionTool {
    make_web_fetch(
        MakeWebFetchOptions::builder()
            .mode(WebFetchMode::Markdown)
            .max_content_chars(max_content_chars)
            .build(),
    )
    .unwrap()
}

fn agentic(model: &Arc<MockMessageModel>) -> FunctionTool {
    let model: Arc<dyn Model> = Arc::clone(model) as Arc<dyn Model>;
    make_web_fetch(MakeWebFetchOptions::builder().model(model).build()).unwrap()
}

fn answering(text: &str) -> Arc<MockMessageModel> {
    let model = Arc::new(MockMessageModel::new());
    model.add_turn(vec![TextBlock::new(text).into()]);
    model
}

async fn fetch(tool: &FunctionTool, input: JsonValue) -> Result<String, String> {
    tool.invoke(input, context(None))
        .await
        .map(|value| value.as_str().unwrap().to_owned())
        .map_err(|error| error.to_string())
}

/// The text of the analyst's first request.
fn analyst_prompt(model: &MockMessageModel) -> String {
    let request = &model.requests()[0];
    let message = &request.messages[0];
    assert_eq!(message.role(), Role::User);
    message
        .content()
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.text()),
            _ => None,
        })
        .collect()
}

mod make_web_fetch_factory {
    use super::*;

    #[test]
    fn defaults_to_agentic_mode() {
        let tool = make_web_fetch(MakeWebFetchOptions::default()).unwrap();
        assert_eq!(tool.description(), WEB_FETCH_DESCRIPTION_AGENTIC);
        assert_eq!(web_fetch().name(), "web_fetch");
        assert_eq!(web_fetch().description(), WEB_FETCH_DESCRIPTION_AGENTIC);
    }

    #[test]
    fn markdown_mode_uses_markdown_description() {
        assert_eq!(markdown().description(), WEB_FETCH_DESCRIPTION_MARKDOWN);
    }

    #[test]
    fn custom_name_and_description_override_defaults() {
        let tool = make_web_fetch(
            MakeWebFetchOptions::builder()
                .name("fetch_page")
                .description("custom desc")
                .build(),
        )
        .unwrap();
        assert_eq!(tool.name(), "fetch_page");
        assert_eq!(tool.description(), "custom desc");
    }

    #[test]
    fn rejects_non_positive_max_bytes() {
        let error = make_web_fetch(MakeWebFetchOptions::builder().max_bytes(0).build()).unwrap_err();
        assert_eq!(error.to_string(), "maxBytes must be a positive number, got 0");
    }

    #[test]
    fn rejects_non_positive_max_content_chars() {
        let error = make_web_fetch(MakeWebFetchOptions::builder().max_content_chars(0).build()).unwrap_err();
        assert_eq!(
            error.to_string(),
            "maxContentChars must be a positive number, got 0"
        );
    }

    #[test]
    fn exports_correct_default_constants() {
        assert_eq!(DEFAULT_MAX_BYTES, 5 * 1024 * 1024);
        assert_eq!(DEFAULT_MAX_CONTENT_CHARS, 50_000);
    }

    #[test]
    fn exposes_the_zod_schemas() {
        let url = json!({ "type": "string", "description": "URL to fetch. Must be http:// or https://." });
        assert_eq!(
            markdown().tool_spec().input_schema(),
            Some(&json!({
                "type": "object",
                "properties": { "url": url },
                "required": ["url"],
                "additionalProperties": false,
            }))
        );
        assert_eq!(
            web_fetch().tool_spec().input_schema(),
            Some(&json!({
                "type": "object",
                "properties": {
                    "url": url,
                    "prompt": { "type": "string", "description": "Question or instruction about the page content." },
                },
                "required": ["url", "prompt"],
                "additionalProperties": false,
            }))
        );
    }
}

mod markdown_mode {
    use super::*;

    #[tokio::test]
    async fn html_response_is_converted_to_markdown() {
        let (url, _) = server(reply("<h1>Hi</h1>", "text/html")).await;
        assert_eq!(fetch(&markdown(), json!({ "url": url })).await.unwrap(), "# Hi\n");
    }

    #[tokio::test]
    async fn xml_content_type_is_also_converted_to_markdown() {
        let (url, _) = server(reply("<p>xhtml</p>", "application/xhtml+xml")).await;
        assert_eq!(
            fetch(&markdown(), json!({ "url": url })).await.unwrap(),
            "xhtml\n"
        );
    }

    #[tokio::test]
    async fn non_html_response_is_returned_as_is() {
        let (url, _) = server(reply("plain <b>text</b> response", "text/plain")).await;
        assert_eq!(
            fetch(&markdown(), json!({ "url": url })).await.unwrap(),
            "plain <b>text</b> response"
        );
    }

    #[tokio::test]
    async fn content_is_truncated_at_max_content_chars() {
        let (url, _) = server(reply(&"x".repeat(200), "text/plain")).await;
        let result = fetch(&markdown_with(50), json!({ "url": url })).await.unwrap();
        assert_eq!(result, format!("{}\n\n[content truncated]", "x".repeat(50)));
    }

    #[tokio::test]
    async fn truncates_by_character_not_byte() {
        let (url, _) = server(reply(&"é".repeat(10), "text/plain; charset=utf-8")).await;
        let result = fetch(&markdown_with(3), json!({ "url": url })).await.unwrap();
        assert_eq!(result, "ééé\n\n[content truncated]");
    }

    #[tokio::test]
    async fn rejects_non_http_scheme() {
        let error = fetch(&markdown(), json!({ "url": "file:///etc/passwd" }))
            .await
            .unwrap_err();
        assert_eq!(
            error,
            "url=<file:///etc/passwd> | fetch failed: only http and https URLs are supported"
        );
    }

    #[tokio::test]
    async fn rejects_invalid_url() {
        let error = fetch(&markdown(), json!({ "url": "not a url" }))
            .await
            .unwrap_err();
        assert_eq!(error, "url=<not a url> | fetch failed: invalid URL");
    }

    #[tokio::test]
    async fn rejects_4xx_status() {
        let (url, _) = server(Reply {
            status: "404 Not Found",
            ..reply("Not Found", "text/plain")
        })
        .await;
        let error = fetch(&markdown(), json!({ "url": url })).await.unwrap_err();
        assert_eq!(error, format!("HTTP 404 Not Found: GET {url}"));
    }

    #[tokio::test]
    async fn rejects_response_body_exceeding_max_bytes() {
        let (url, _) = server(Reply {
            content_type: None,
            body: vec![0; DEFAULT_MAX_BYTES + 1],
            ..reply("", "")
        })
        .await;
        let error = fetch(&markdown(), json!({ "url": url })).await.unwrap_err();
        assert_eq!(
            error,
            "Response body exceeded max_bytes=5242880. Refusing to buffer more."
        );
    }

    #[tokio::test]
    async fn wraps_network_errors() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/", listener.local_addr().unwrap());
        drop(listener);
        let error = fetch(&markdown(), json!({ "url": url })).await.unwrap_err();
        assert!(
            error.starts_with(&format!("url=<{url}> | fetch failed: ")),
            "{error}"
        );
    }

    #[tokio::test]
    async fn throws_on_abort_signal() {
        let (url, _) = server(reply("ok", "text/plain")).await;
        let (context, handle) = cancellable_context(None);
        handle.cancel(crate::CancelOptions::default());
        let error = markdown()
            .invoke(json!({ "url": url }), context)
            .await
            .unwrap_err();
        assert_eq!(error.to_string(), "Web fetch tool request cancelled");
    }

    #[tokio::test]
    async fn decodes_response_body_using_charset_from_content_type() {
        let (url, _) = server(Reply {
            body: vec![0x3c, 0x70, 0x3e, 0xa9, 0x3c, 0x2f, 0x70, 0x3e],
            ..reply("", "text/plain; charset=iso-8859-1")
        })
        .await;
        assert_eq!(
            fetch(&markdown(), json!({ "url": url })).await.unwrap(),
            "<p>©</p>"
        );
    }

    #[tokio::test]
    async fn aborts_mid_body_read_when_the_stream_reader_throws_an_abort_error() {
        let (url, _, written) = notifying_server(Reply {
            hang: true,
            ..reply("a", "text/plain")
        })
        .await;
        let (context, handle) = cancellable_context(None);
        let tool = markdown();
        let call = tool.invoke(json!({ "url": url }), context);
        let cancel = async {
            written.notified().await;
            handle.cancel(crate::CancelOptions::default());
        };
        let (result, ()) = tokio::join!(call, cancel);
        assert_eq!(
            result.unwrap_err().to_string(),
            "Web fetch tool request cancelled"
        );
    }

    #[tokio::test]
    async fn sends_correct_user_agent_header_and_passes_cancel_signal() {
        let (url, requests) = server(reply("ok", "text/plain")).await;
        fetch(&markdown(), json!({ "url": url })).await.unwrap();
        let request = requests.lock().unwrap()[0].to_lowercase();
        assert!(request.starts_with("get / http/1.1"), "{request}");
        assert!(
            request.contains("user-agent: strands-agents-web-fetch/1.0"),
            "{request}"
        );
        assert!(
            request.contains("accept: text/html,application/xhtml+xml;q=0.9,*/*;q=0.8"),
            "{request}"
        );
    }
}

mod agentic_mode {
    use super::*;

    #[tokio::test]
    async fn requires_a_non_empty_prompt() {
        let (url, requests) = server(reply("<p>content</p>", "text/html")).await;
        let model = answering("unused");
        let error = fetch(&agentic(&model), json!({ "url": url, "prompt": "   " }))
            .await
            .unwrap_err();
        assert_eq!(error, "web_fetch: agentic mode requires a non-empty prompt.");
        assert!(requests.lock().unwrap().is_empty());
        assert!(model.requests().is_empty());
    }

    #[tokio::test]
    async fn uses_factory_model_when_provided() {
        let (url, _) = server(reply("<p>page content</p>", "text/html")).await;
        let model = answering("the answer");
        let mut agent = LocalAgent::new(answering("host model") as Arc<dyn Model>);
        agent.set_aux_model(Some(answering("host aux model") as Arc<dyn Model>));
        let result = agentic(&model)
            .invoke(
                json!({ "url": url, "prompt": "What is this?" }),
                context_for(&agent, "web_fetch", JsonValue::Null),
            )
            .await
            .unwrap();
        assert_eq!(result, json!("the answer"));
        assert_eq!(model.requests().len(), 1);
    }

    #[tokio::test]
    async fn falls_back_to_host_agent_aux_model() {
        let (url, _) = server(reply("<p>page content</p>", "text/html")).await;
        let host_model = answering("host answer");
        let host_aux_model = answering("aux answer");
        let mut agent = LocalAgent::new(Arc::clone(&host_model) as Arc<dyn Model>);
        agent.set_aux_model(Some(Arc::clone(&host_aux_model) as Arc<dyn Model>));
        let tool = make_web_fetch(MakeWebFetchOptions::default()).unwrap();
        let result = tool
            .invoke(
                json!({ "url": url, "prompt": "Summarize" }),
                context_for(&agent, "web_fetch", JsonValue::Null),
            )
            .await
            .unwrap();
        assert_eq!(result, json!("aux answer"));
        assert_eq!(
            (host_model.requests().len(), host_aux_model.requests().len()),
            (0, 1)
        );
    }

    #[tokio::test]
    async fn falls_back_to_host_agent_model_without_an_aux_model() {
        let (url, _) = server(reply("page content", "text/plain")).await;
        let host_model = answering("host answer");
        let agent = LocalAgent::new(Arc::clone(&host_model) as Arc<dyn Model>);
        let result = web_fetch()
            .invoke(
                json!({ "url": url, "prompt": "Summarize" }),
                context_for(&agent, "web_fetch", JsonValue::Null),
            )
            .await
            .unwrap();
        assert_eq!(result, json!("host answer"));
    }

    #[tokio::test]
    async fn passes_prompt_and_page_content_to_analyst() {
        let (url, _) = server(reply("page content", "text/plain")).await;
        let model = answering("the answer");
        let result = fetch(
            &agentic(&model),
            json!({ "url": url, "prompt": "What is this about?" }),
        )
        .await
        .unwrap();
        assert_eq!(result, "the answer");
        assert_eq!(
            analyst_prompt(&model),
            format!("URL: {url}\n\nRequest: What is this about?\n\n--- Content ---\npage content")
        );
        let request = &model.requests()[0];
        assert_eq!(
            request
                .system_prompt
                .as_ref()
                .map(|prompt| serde_json::to_value(prompt).unwrap()),
            Some(json!(ANALYST_PROMPT))
        );
    }

    #[tokio::test]
    async fn strips_reasoning_blocks_from_analyst_result() {
        let (url, _) = server(reply("page content", "text/plain")).await;
        let model = Arc::new(MockMessageModel::new());
        model.add_turn(vec![
            ReasoningBlock::new()
                .with_text("internal chain-of-thought")
                .into(),
            TextBlock::new("the answer").into(),
        ]);
        let result = fetch(&agentic(&model), json!({ "url": url, "prompt": "Summarize" }))
            .await
            .unwrap();
        assert_eq!(result, "the answer");
    }

    #[tokio::test]
    async fn truncates_content_before_passing_to_analyst() {
        let (url, _) = server(reply(&"x".repeat(200), "text/plain")).await;
        let model = answering("answer");
        let model_dyn: Arc<dyn Model> = Arc::clone(&model) as Arc<dyn Model>;
        let tool = make_web_fetch(
            MakeWebFetchOptions::builder()
                .model(model_dyn)
                .max_content_chars(50)
                .build(),
        )
        .unwrap();
        fetch(&tool, json!({ "url": url, "prompt": "Summarize" }))
            .await
            .unwrap();
        let prompt = analyst_prompt(&model);
        assert!(prompt.ends_with(&format!("{}\n\n[content truncated]", "x".repeat(50))));
        assert!(!prompt.contains(&"x".repeat(51)));
    }

    #[tokio::test]
    async fn wraps_analyst_error_with_url_context() {
        let (url, _) = server(reply("<p>content</p>", "text/html")).await;
        let model = Arc::new(MockMessageModel::new());
        model.add_error(|| Error::other("analyst boom"));
        let error = fetch(&agentic(&model), json!({ "url": url, "prompt": "Summarize" }))
            .await
            .unwrap_err();
        assert!(
            error.starts_with(&format!("url=<{url}> | web fetch analyst failed: ")),
            "{error}"
        );
        assert!(error.contains("analyst boom"), "{error}");
    }

    mod request_limits {
        use super::*;

        #[tokio::test]
        async fn adds_the_analyst_usage_to_the_request_total_and_runs_it_past_a_spent_limit() {
            let (url, _) = server(reply("page content", "text/plain")).await;
            let model = Arc::new(MockMessageModel::new());
            model.add_turn_with(
                vec![TextBlock::new("the answer").into()],
                MockTurnOptions {
                    usage: Some(Usage::new(20, 6, 26)),
                    ..MockTurnOptions::default()
                },
            );
            let limits = InvokeLimits {
                turns: NonZeroU32::new(1),
                ..InvokeLimits::default()
            };
            let parent = InvocationContext::new(Some(limits), None, false);
            parent.set_turns_taken(5);
            let mut agent = LocalAgent::new(Arc::new(MockMessageModel::new()));
            agent.set_invocation_context(Some(parent.clone()));

            let result = agentic(&model)
                .invoke(
                    json!({ "url": url, "prompt": "Summarize" }),
                    context_for(&agent, "web_fetch", JsonValue::Null),
                )
                .await
                .unwrap();

            assert_eq!(result, json!("the answer"));
            let usage = parent.usage();
            assert_eq!(
                (
                    usage.turns_taken(),
                    usage.tokens().input_tokens(),
                    usage.tokens().output_tokens(),
                    usage.tokens().total_tokens()
                ),
                (5, 20, 6, 26)
            );
        }
    }
}
