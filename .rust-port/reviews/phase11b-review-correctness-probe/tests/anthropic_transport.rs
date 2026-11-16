use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use strands::Agent;
use strands::models::anthropic::{AnthropicClientConfig, AnthropicModel, AnthropicModelOptions};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const HEAD: &str = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ntransfer-encoding: chunked\r\n\r\n";

fn sse(event: &str, data: &str) -> String {
    format!("event: {event}\ndata: {data}\n\n")
}

fn start() -> String {
    sse("message_start", r#"{"type":"message_start","message":{"id":"m","type":"message","role":"assistant","content":[],"model":"x","usage":{"input_tokens":1,"output_tokens":0}}}"#)
        + &sse("content_block_start", r#"{"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}}"#)
}

fn delta(text: &str) -> String {
    sse("content_block_delta", &format!(r#"{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"{text}"}}}}"#))
}

fn finish() -> String {
    sse("content_block_stop", r#"{"type":"content_block_stop","index":0}"#)
        + &sse("message_delta", r#"{"type":"message_delta","delta":{"stop_reason":"end_turn"},"usage":{"output_tokens":2}}"#)
        + &sse("message_stop", r#"{"type":"message_stop"}"#)
}

async fn chunk(socket: &mut TcpStream, bytes: &[u8]) {
    let _ = socket.write_all(format!("{:x}\r\n", bytes.len()).as_bytes()).await;
    let _ = socket.write_all(bytes).await;
    let _ = socket.write_all(b"\r\n").await;
    let _ = socket.flush().await;
}

async fn read_request(socket: &mut TcpStream) {
    let mut buffer = vec![0u8; 65536];
    let mut seen = Vec::new();
    loop {
        let Ok(n) = socket.read(&mut buffer).await else { return };
        if n == 0 { return; }
        seen.extend_from_slice(&buffer[..n]);
        if let Some(head_end) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&seen[..head_end]).to_ascii_lowercase();
            let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).map_or(0, |v| v.trim().parse().unwrap_or(0));
            if seen.len() >= head_end + 4 + length { return; }
        }
    }
}

async fn serve(scenario: &'static str) -> (String, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let hits = Arc::new(AtomicUsize::new(0));
    let counter = Arc::clone(&hits);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let hit = counter.fetch_add(1, Ordering::SeqCst);
            tokio::spawn(async move {
                read_request(&mut socket).await;
                match scenario {
                    "ok" => {
                        socket.write_all(HEAD.as_bytes()).await.unwrap();
                        chunk(&mut socket, (start() + &delta("Hi") + &finish()).as_bytes()).await;
                        let _ = socket.write_all(b"0\r\n\r\n").await;
                    }
                    "abrupt" => {
                        socket.write_all(HEAD.as_bytes()).await.unwrap();
                        chunk(&mut socket, (start() + &delta("Hel")).as_bytes()).await;
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        drop(socket);
                    }
                    "clean_end_without_stop" => {
                        socket.write_all(HEAD.as_bytes()).await.unwrap();
                        chunk(&mut socket, (start() + &delta("Hel")).as_bytes()).await;
                        let _ = socket.write_all(b"0\r\n\r\n").await;
                    }
                    "split_utf8" => {
                        socket.write_all(HEAD.as_bytes()).await.unwrap();
                        let body = (start() + &delta("H\u{e9}\u{1F600}!") + &finish()).into_bytes();
                        for byte in body {
                            chunk(&mut socket, &[byte]).await;
                        }
                        let _ = socket.write_all(b"0\r\n\r\n").await;
                    }
                    "html_502" => {
                        let body = "<html>bad gateway</html>";
                        let _ = socket.write_all(format!("HTTP/1.1 502 Bad Gateway\r\ncontent-type: text/html\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
                    }
                    "json_200" => {
                        let body = r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#;
                        let _ = socket.write_all(format!("HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\n\r\n{body}", body.len()).as_bytes()).await;
                    }
                    "error_event" => {
                        socket.write_all(HEAD.as_bytes()).await.unwrap();
                        chunk(&mut socket, (start() + &sse("error", r#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#)).as_bytes()).await;
                        let _ = socket.write_all(b"0\r\n\r\n").await;
                    }
                    "silent" => {
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    }
                    "retry_after_8s" => {
                        if hit == 0 {
                            let _ = socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nretry-after: 8\r\ncontent-length: 2\r\n\r\n{}").await;
                        } else {
                            socket.write_all(HEAD.as_bytes()).await.unwrap();
                            chunk(&mut socket, (start() + &delta("Hi") + &finish()).as_bytes()).await;
                            let _ = socket.write_all(b"0\r\n\r\n").await;
                        }
                    }
                    "retry_then_ok" => {
                        if hit == 0 {
                            let _ = socket.write_all(b"HTTP/1.1 429 Too Many Requests\r\nretry-after-ms: 50\r\ncontent-length: 2\r\n\r\n{}").await;
                        } else {
                            socket.write_all(HEAD.as_bytes()).await.unwrap();
                            chunk(&mut socket, (start() + &delta("Hi") + &finish()).as_bytes()).await;
                            let _ = socket.write_all(b"0\r\n\r\n").await;
                        }
                    }
                    _ => {}
                }
            });
        }
    });
    (url, hits)
}

async fn run(scenario: &'static str, max_retries: u32, timeout: Option<Duration>) -> String {
    let (url, hits) = serve(scenario).await;
    let model = AnthropicModel::new(AnthropicModelOptions {
        api_key: Some("k".to_owned()),
        client_config: AnthropicClientConfig { base_url: Some(url), max_retries: Some(max_retries), timeout, ..Default::default() },
        ..Default::default()
    })
    .unwrap();
    let mut agent = Agent::builder().model(model).no_retry_strategy().printer(false).build().unwrap();
    let started = Instant::now();
    let outcome = tokio::time::timeout(Duration::from_secs(20), agent.invoke("hi")).await;
    let text = match outcome {
        Err(_) => "HUNG (20 s)".to_owned(),
        Ok(Ok(result)) => format!("ok stop={:?} text={:?}", result.stop_reason(), result.to_string()),
        Ok(Err(error)) => format!("err {error:?}"),
    };
    let line = format!("{scenario}: {:.2}s requests={} messages={} -> {}", started.elapsed().as_secs_f32(), hits.load(Ordering::SeqCst), agent.messages().len(), text.chars().take(400).collect::<String>());
    eprintln!("{line}");
    line
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn scenarios() {
    run("ok", 0, None).await;
    run("abrupt", 0, None).await;
    run("clean_end_without_stop", 0, None).await;
    run("split_utf8", 0, None).await;
    run("html_502", 0, None).await;
    run("json_200", 0, None).await;
    run("error_event", 0, None).await;
    run("silent", 1, Some(Duration::from_millis(300))).await;
    run("retry_then_ok", 2, None).await;
}

// Finding 5 on the real adapter: a cancel during the transport's retry sleep waits the sleep out.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_during_the_retry_after_sleep() {
    let (url, hits) = serve("retry_after_8s").await;
    let model = AnthropicModel::new(AnthropicModelOptions {
        api_key: Some("k".to_owned()),
        client_config: AnthropicClientConfig { base_url: Some(url), max_retries: Some(2), ..Default::default() },
        ..Default::default()
    })
    .unwrap();
    let mut agent = Agent::builder().model(model).no_retry_strategy().printer(false).build().unwrap();
    let handle = agent.cancel_handle();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.cancel(strands::CancelOptions::default());
    });
    let started = Instant::now();
    let result = agent.invoke("hi").await.unwrap();
    let elapsed = started.elapsed();
    eprintln!("cancel at 0.3s: returned after {:.2}s stop={:?} requests={}", elapsed.as_secs_f32(), result.stop_reason(), hits.load(Ordering::SeqCst));
    assert!(elapsed < Duration::from_secs(2), "the cancel waited {elapsed:?} for the retry-after sleep");
}
