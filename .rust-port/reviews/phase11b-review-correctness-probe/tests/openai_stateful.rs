use std::sync::{Arc, Mutex};
use std::time::Duration;

use strands::Agent;
use strands::models::openai::{OpenAIClientConfig, OpenAIModel, OpenAIModelConfig, OpenAIModelOptions};
use strands::session::{SessionManager, SessionManagerConfig};
use strands::storage::{InMemoryStorage, Storage};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

async fn read_request(socket: &mut TcpStream) -> String {
    let mut buffer = vec![0u8; 65536];
    let mut seen = Vec::new();
    loop {
        let Ok(n) = socket.read(&mut buffer).await else { return String::new() };
        if n == 0 { return String::new(); }
        seen.extend_from_slice(&buffer[..n]);
        if let Some(head_end) = seen.windows(4).position(|w| w == b"\r\n\r\n") {
            let head = String::from_utf8_lossy(&seen[..head_end]).to_ascii_lowercase();
            let length: usize = head.lines().find_map(|l| l.strip_prefix("content-length:")).map_or(0, |v| v.trim().parse().unwrap_or(0));
            if seen.len() >= head_end + 4 + length {
                return String::from_utf8_lossy(&seen[head_end + 4..]).into_owned();
            }
        }
    }
}

async fn serve() -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let bodies = Arc::new(Mutex::new(Vec::new()));
    let seen = Arc::clone(&bodies);
    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else { return };
            let seen = Arc::clone(&seen);
            tokio::spawn(async move {
                let body = read_request(&mut socket).await;
                let slow = body.contains("slow");
                let n = {
                    let mut seen = seen.lock().unwrap();
                    seen.push(body);
                    seen.len()
                };
                if slow {
                    tokio::time::sleep(Duration::from_secs(5)).await;
                }
                let events = [
                    format!(r#"{{"type":"response.created","response":{{"id":"resp_{n}","status":"in_progress"}}}}"#),
                    r#"{"type":"response.output_item.added","output_index":0,"item":{"type":"message","id":"msg_1","role":"assistant","content":[]}}"#.to_owned(),
                    format!(r#"{{"type":"response.output_text.delta","delta":"answer {n}","output_index":0,"content_index":0,"item_id":"msg_1"}}"#),
                    format!(r#"{{"type":"response.completed","response":{{"id":"resp_{n}","status":"completed","usage":{{"input_tokens":1,"output_tokens":1,"total_tokens":2}}}}}}"#),
                ];
                let payload: String = events.iter().map(|event| format!("data: {event}\n\n")).collect();
                let reply = format!("HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{payload}", payload.len());
                let _ = socket.write_all(reply.as_bytes()).await;
            });
        }
    });
    (url, bodies)
}

fn agent(url: &str, storage: &Arc<dyn Storage>) -> Agent {
    let model = OpenAIModel::new(OpenAIModelOptions {
        api_key: Some("k".into()),
        client_config: OpenAIClientConfig { base_url: Some(url.to_owned()), max_retries: Some(0), ..Default::default() },
        config: OpenAIModelConfig { stateful: Some(true), ..Default::default() },
        ..Default::default()
    })
    .unwrap();
    let session = SessionManager::new(SessionManagerConfig { session_id: Some("s1".to_owned()), ..Default::default() }).unwrap();
    Agent::builder().id("a1").model(model).storage(Arc::clone(storage)).session_manager(session).printer(false).build().unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_stateful_responses_conversation_survives_a_restart() {
    let (url, bodies) = serve().await;
    let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
    let mut first = agent(&url, &storage);
    let one = tokio::time::timeout(Duration::from_secs(10), first.invoke("first question")).await.unwrap().unwrap();
    let two = first.invoke("second question").await.unwrap();
    eprintln!("first agent: {one} / {two}; messages={}", first.messages().len());
    first.shutdown().await.unwrap();
    drop(first);
    let mut second = agent(&url, &storage);
    let three = second.invoke("third question").await.unwrap();
    eprintln!("second agent: {three}; messages={}", second.messages().len());
    for (index, body) in bodies.lock().unwrap().iter().enumerate() {
        let value: serde_json::Value = serde_json::from_str(body).unwrap();
        eprintln!("request {}: previous_response_id={} store={} input={}", index + 1, value["previous_response_id"], value["store"], value["input"]);
    }
    let bodies = bodies.lock().unwrap();
    let third: serde_json::Value = serde_json::from_str(&bodies[2]).unwrap();
    assert_eq!(third["previous_response_id"], "resp_2");
    assert_eq!(third["input"].as_array().map(Vec::len), Some(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn stateful_history_after_a_drop_and_after_a_cancel() {
    for mode in ["drop", "cancel"] {
        let (url, bodies) = serve().await;
        let storage: Arc<dyn Storage> = Arc::new(InMemoryStorage::new());
        let mut agent = agent(&url, &storage);
        agent.invoke("first question").await.unwrap();
        if mode == "drop" {
            assert!(tokio::time::timeout(Duration::from_millis(300), agent.invoke("slow question")).await.is_err());
        } else {
            let handle = agent.cancel_handle();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(300)).await;
                handle.cancel(strands::CancelOptions::default());
            });
            let result = agent.invoke("slow question").await;
            eprintln!("[{mode}] cancelled result: {:?}", result.map(|r| r.stop_reason()).map_err(|e| e.to_string()));
        }
        eprintln!("[{mode}] messages after: {}", serde_json::to_string(&agent.messages()).unwrap());
        let after = agent.invoke("after").await.map(|r| r.to_string()).map_err(|e| e.to_string());
        eprintln!("[{mode}] next invoke: {after:?}; messages={}", agent.messages().len());
        let bodies = bodies.lock().unwrap();
        let last: serde_json::Value = serde_json::from_str(bodies.last().unwrap()).unwrap();
        eprintln!("[{mode}] request {}: previous_response_id={} input={}", bodies.len(), last["previous_response_id"], last["input"]);
        drop(bodies);
        agent.shutdown().await.unwrap();
    }
}
