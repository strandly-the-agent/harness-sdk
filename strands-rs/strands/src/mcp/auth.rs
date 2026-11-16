//! OAuth client credentials for the url-configured client (TS `ClientCredentialsProvider`).

use std::collections::HashMap;
use std::future::Future;
use std::sync::Arc;

use http::{HeaderName, HeaderValue};
use rmcp::model::ClientJsonRpcMessage;
use rmcp::transport::AuthError;
use rmcp::transport::common::client_side_sse::BoxedSseResponse;
use rmcp::transport::streamable_http_client::{
    StreamableHttpClient, StreamableHttpError, StreamableHttpPostResponse,
};

use super::client::{McpAuthProvider, McpClientCredentials};

type HttpError = StreamableHttpError<reqwest::Error>;

/// The client-credentials grant against the server's discovered authorization server; scopes
/// are sent space-separated and the server url is the RFC 8707 `resource`, which `rmcp` requires.
pub(crate) async fn client_credentials(
    url: &str,
    credentials: McpClientCredentials,
) -> Result<McpAuthProvider, AuthError> {
    let mut state = rmcp::transport::auth::OAuthState::new(url, None).await?;
    let config = rmcp::transport::ClientCredentialsConfig::ClientSecret {
        client_id: credentials.client_id,
        client_secret: credentials.client_secret,
        scopes: credentials.scopes.unwrap_or_default(),
        resource: Some(url.to_owned()),
    };
    state.authenticate_client_credentials(config).await?;
    let manager = state.into_authorization_manager().ok_or_else(|| {
        AuthError::AuthorizationFailed("OAuth client credentials did not authorize".to_owned())
    })?;
    Ok(rmcp::transport::AuthClient::new(
        reqwest::Client::default(),
        manager,
    ))
}

/// An HTTP client that runs the client-credentials grant again when its token expires or the
/// server rejects it, as the MCP SDK's non-interactive providers do; client-credentials tokens
/// come without a refresh token, so `rmcp`'s own refresh can't renew them.
#[derive(Clone)]
pub(crate) struct ClientCredentialsClient {
    url: Arc<str>,
    credentials: McpClientCredentials,
    current: Arc<tokio::sync::Mutex<McpAuthProvider>>,
}

impl ClientCredentialsClient {
    pub(crate) fn new(url: &str, credentials: McpClientCredentials, provider: McpAuthProvider) -> Self {
        Self {
            url: url.into(),
            credentials,
            current: Arc::new(tokio::sync::Mutex::new(provider)),
        }
    }

    /// The provider and a usable token, granting a new one when the stored token is gone.
    async fn authorized(&self) -> Result<(McpAuthProvider, String), HttpError> {
        let mut current = self.current.lock().await;
        match current.get_access_token().await {
            Ok(token) => return Ok((current.clone(), token)),
            Err(AuthError::AuthorizationRequired) => {}
            Err(error) => return Err(error.into()),
        }
        *current = client_credentials(&self.url, self.credentials.clone()).await?;
        Ok((current.clone(), granted_token(&current).await?))
    }

    /// Grants a new token after the server rejected the one `used` sent, unless a concurrent
    /// request already replaced it.
    async fn regrant(&self, used: &McpAuthProvider) -> Result<(McpAuthProvider, String), HttpError> {
        let mut current = self.current.lock().await;
        if Arc::ptr_eq(&current.auth_manager, &used.auth_manager) {
            *current = client_credentials(&self.url, self.credentials.clone()).await?;
        }
        Ok((current.clone(), granted_token(&current).await?))
    }

    async fn call<T, F, Fut>(&self, call: F) -> Result<T, HttpError>
    where
        F: Fn(McpAuthProvider, String) -> Fut,
        Fut: Future<Output = Result<T, HttpError>>,
    {
        let (provider, token) = self.authorized().await?;
        match call(provider.clone(), token).await {
            Err(StreamableHttpError::AuthRequired(challenge)) => match self.regrant(&provider).await {
                Ok((provider, token)) => call(provider, token).await,
                Err(_) => Err(StreamableHttpError::AuthRequired(challenge)),
            },
            result => result,
        }
    }
}

/// The token of the latest grant, read without `rmcp`'s 30-second expiry margin, so a server
/// that issues short-lived tokens still gets the token it just granted.
async fn granted_token(provider: &McpAuthProvider) -> Result<String, AuthError> {
    let (_, response) = provider.auth_manager.lock().await.get_credentials().await?;
    serde_json::to_value(response)
        .ok()
        .and_then(|response| response.get("access_token")?.as_str().map(str::to_owned))
        .ok_or(AuthError::AuthorizationRequired)
}

impl StreamableHttpClient for ClientCredentialsClient {
    type Error = reqwest::Error;

    async fn post_message(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        self.call(|provider, token| {
            let (uri, message, session_id, headers) = (
                uri.clone(),
                message.clone(),
                session_id.clone(),
                custom_headers.clone(),
            );
            async move {
                provider
                    .post_message(uri, message, session_id, Some(token), headers)
                    .await
            }
        })
        .await
    }

    async fn post_message_with_max_sse_event_size(
        &self,
        uri: Arc<str>,
        message: ClientJsonRpcMessage,
        session_id: Option<Arc<str>>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
        max_sse_event_size: usize,
    ) -> Result<StreamableHttpPostResponse, HttpError> {
        self.call(|provider, token| {
            let (uri, message, session_id, headers) = (
                uri.clone(),
                message.clone(),
                session_id.clone(),
                custom_headers.clone(),
            );
            async move {
                provider
                    .post_message_with_max_sse_event_size(
                        uri,
                        message,
                        session_id,
                        Some(token),
                        headers,
                        max_sse_event_size,
                    )
                    .await
            }
        })
        .await
    }

    async fn delete_session(
        &self,
        uri: Arc<str>,
        session_id: Arc<str>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<(), HttpError> {
        self.call(|provider, token| {
            let (uri, session_id, headers) = (uri.clone(), session_id.clone(), custom_headers.clone());
            async move {
                provider
                    .delete_session(uri, session_id, Some(token), headers)
                    .await
            }
        })
        .await
    }

    async fn get_stream(
        &self,
        uri: Arc<str>,
        session_id: Option<Arc<str>>,
        last_event_id: Option<String>,
        _auth_header: Option<String>,
        custom_headers: HashMap<HeaderName, HeaderValue>,
    ) -> Result<BoxedSseResponse, HttpError> {
        self.call(|provider, token| {
            let (uri, session_id, last_event_id, headers) = (
                uri.clone(),
                session_id.clone(),
                last_event_id.clone(),
                custom_headers.clone(),
            );
            async move {
                provider
                    .get_stream(uri, session_id, last_event_id, Some(token), headers)
                    .await
            }
        })
        .await
    }
}
