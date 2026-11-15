//! The legacy HTTP+SSE client transport (protocol revision 2024-11-05; the MCP SDK
//! `SSEClientTransport`), which `rmcp` doesn't ship.
//!
//! The client opens a `GET` event stream; the server's first `endpoint` event names the url the
//! client `POST`s its messages to, and later `message` events carry the server's messages.

use std::borrow::Cow;

use eventsource_stream::Eventsource;
use futures::StreamExt;
use http::HeaderMap;
use rmcp::RoleClient;
use rmcp::service::{RxJsonRpcMessage, TxJsonRpcMessage};
use rmcp::transport::Transport;
use tokio::sync::{mpsc, watch};
use tracing::Instrument;

/// A legacy SSE transport failure.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub(crate) struct SseTransportError(String);

#[derive(Clone)]
enum Endpoint {
    Pending,
    Ready(reqwest::Url),
    Failed(String),
}

pub(crate) struct SseClientTransport {
    client: reqwest::Client,
    headers: HeaderMap,
    endpoint: watch::Receiver<Endpoint>,
    messages: mpsc::Receiver<RxJsonRpcMessage<RoleClient>>,
    reader: tokio::task::JoinHandle<()>,
}

impl SseClientTransport {
    /// Opens the event stream on a background task; must run inside a Tokio runtime.
    pub(crate) fn start(url: reqwest::Url, headers: HeaderMap) -> Self {
        let client = reqwest::Client::new();
        let (endpoint_tx, endpoint) = watch::channel(Endpoint::Pending);
        let (messages_tx, messages) = mpsc::channel(64);
        let reader = tokio::spawn(
            read_events(client.clone(), url, headers.clone(), endpoint_tx, messages_tx).in_current_span(),
        );
        Self {
            client,
            headers,
            endpoint,
            messages,
            reader,
        }
    }
}

impl Drop for SseClientTransport {
    fn drop(&mut self) {
        self.reader.abort();
    }
}

async fn read_events(
    client: reqwest::Client,
    url: reqwest::Url,
    headers: HeaderMap,
    endpoint: watch::Sender<Endpoint>,
    messages: mpsc::Sender<RxJsonRpcMessage<RoleClient>>,
) {
    let response = client
        .get(url.clone())
        .headers(headers)
        .header(http::header::ACCEPT, "text/event-stream")
        .send()
        .await;
    let response = match response {
        Ok(response) if response.status().is_success() => response,
        Ok(response) => {
            let status = response.status().as_u16();
            endpoint.send_replace(Endpoint::Failed(format!(
                "SSE error: Non-200 status code ({status})"
            )));
            return;
        }
        Err(error) => {
            endpoint.send_replace(Endpoint::Failed(format!("SSE error: {error}")));
            return;
        }
    };
    let mut events = response.bytes_stream().eventsource();
    while let Some(event) = events.next().await {
        let Ok(event) = event else { break };
        match event.event.as_str() {
            "endpoint" => {
                endpoint.send_replace(resolve_endpoint(&url, &event.data));
            }
            "" | "message" => match serde_json::from_str::<RxJsonRpcMessage<RoleClient>>(&event.data) {
                Ok(message) => {
                    if messages.send(message).await.is_err() {
                        break;
                    }
                }
                Err(error) => tracing::warn!("error=<{error}> | invalid mcp sse message, skipping"),
            },
            _ => {}
        }
    }
    if matches!(*endpoint.borrow(), Endpoint::Pending) {
        endpoint.send_replace(Endpoint::Failed(
            "SSE connection closed before the endpoint event".to_owned(),
        ));
    }
}

/// The POST url, which must share the event stream's origin (as the MCP SDK checks).
fn resolve_endpoint(url: &reqwest::Url, data: &str) -> Endpoint {
    let Ok(endpoint) = url.join(data) else {
        return Endpoint::Failed(format!("Invalid endpoint url: {data}"));
    };
    if endpoint.origin() != url.origin() {
        return Endpoint::Failed(format!(
            "Endpoint origin does not match connection origin: {}",
            endpoint.origin().ascii_serialization()
        ));
    }
    Endpoint::Ready(endpoint)
}

async fn ready_endpoint(mut endpoint: watch::Receiver<Endpoint>) -> Result<reqwest::Url, SseTransportError> {
    loop {
        match &*endpoint.borrow_and_update() {
            Endpoint::Ready(url) => return Ok(url.clone()),
            Endpoint::Failed(message) => return Err(SseTransportError(message.clone())),
            Endpoint::Pending => {}
        }
        if endpoint.changed().await.is_err() {
            return Err(SseTransportError("SSE connection closed".to_owned()));
        }
    }
}

impl Transport<RoleClient> for SseClientTransport {
    type Error = SseTransportError;

    fn name() -> Cow<'static, str> {
        Cow::Borrowed("SseClientTransport")
    }

    fn send(
        &mut self,
        item: TxJsonRpcMessage<RoleClient>,
    ) -> impl Future<Output = Result<(), Self::Error>> + Send + 'static {
        let client = self.client.clone();
        let headers = self.headers.clone();
        let endpoint = self.endpoint.clone();
        async move {
            let url = ready_endpoint(endpoint).await?;
            let response = client
                .post(url)
                .headers(headers)
                .json(&item)
                .send()
                .await
                .map_err(|error| SseTransportError(format!("SSE error: {error}")))?;
            if response.status().is_success() {
                return Ok(());
            }
            let status = response.status().as_u16();
            let text = response.text().await.unwrap_or_default();
            Err(SseTransportError(format!(
                "Error POSTing to endpoint (HTTP {status}): {text}"
            )))
        }
    }

    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleClient>> {
        self.messages.recv().await
    }

    async fn close(&mut self) -> Result<(), Self::Error> {
        self.reader.abort();
        self.messages.close();
        Ok(())
    }
}
