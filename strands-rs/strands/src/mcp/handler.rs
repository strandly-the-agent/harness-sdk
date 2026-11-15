//! The `rmcp` client handler: identity and capabilities, and the server-to-client requests and
//! notifications the TS client wires through its SDK `Client`.
#![expect(
    deprecated,
    reason = "MCP logging (SEP-2577) is deprecated in rmcp but part of the TS client"
)]

use std::sync::{Arc, Weak};

use rmcp::model::{
    ClientCapabilities, ClientConfig, ElicitRequestParams, ElicitResult, Implementation, LoggingLevel,
    LoggingMessageNotificationParam, ProgressNotificationParam,
};
use rmcp::service::{NotificationContext, RequestContext};
use rmcp::{ClientHandler, ErrorData, RoleClient};

use super::client::{Inner, McpClient, McpProgress};
use super::elicitation::ElicitationContext;
use crate::cancel::{CancelOptions, CancelSignal};

/// Routes server traffic to the owning client; holds it weakly so the connection doesn't keep the
/// client alive.
pub(crate) struct McpHandler {
    pub(crate) client: Weak<Inner>,
    info: ClientConfig,
}

impl McpHandler {
    pub(crate) fn new(
        client: Weak<Inner>,
        name: &str,
        version: &str,
        capabilities: ClientCapabilities,
    ) -> Self {
        let mut implementation = Implementation::default();
        implementation.name = name.to_owned();
        implementation.version = version.to_owned();
        Self {
            client,
            info: ClientConfig::new(capabilities, implementation),
        }
    }

    fn client(&self) -> Option<McpClient> {
        self.client.upgrade().map(|inner| McpClient { inner })
    }
}

/// The `tracing` level for an MCP log level (TS `defaultLogHandler`).
pub(crate) fn log_level(level: LoggingLevel) -> tracing::Level {
    match level {
        LoggingLevel::Debug => tracing::Level::DEBUG,
        LoggingLevel::Info | LoggingLevel::Notice => tracing::Level::INFO,
        LoggingLevel::Warning => tracing::Level::WARN,
        LoggingLevel::Error | LoggingLevel::Critical | LoggingLevel::Alert | LoggingLevel::Emergency => {
            tracing::Level::ERROR
        }
    }
}

/// The TS default log text: `logger=<name>, data=<json> | MCP server log`.
pub(crate) fn log_message(params: &LoggingMessageNotificationParam) -> String {
    format!(
        "logger=<{}>, data=<{}> | MCP server log",
        params.logger.as_deref().unwrap_or("mcp"),
        params.data
    )
}

fn default_log_handler(params: &LoggingMessageNotificationParam) {
    let message = log_message(params);
    match log_level(params.level) {
        tracing::Level::DEBUG => tracing::debug!("{message}"),
        tracing::Level::INFO => tracing::info!("{message}"),
        tracing::Level::WARN => tracing::warn!("{message}"),
        _ => tracing::error!("{message}"),
    }
}

impl ClientHandler for McpHandler {
    fn get_info(&self) -> ClientConfig {
        self.info.clone()
    }

    async fn create_elicitation(
        &self,
        request: ElicitRequestParams,
        context: RequestContext<RoleClient>,
    ) -> Result<ElicitResult, ErrorData> {
        let callback = self
            .client
            .upgrade()
            .and_then(|inner| inner.options.elicitation_callback.clone());
        let Some(callback) = callback else {
            return Err(ErrorData::method_not_found::<
                rmcp::model::ElicitationCreateRequestMethod,
            >());
        };
        let (signal, handle) = CancelSignal::new();
        let answer = callback(ElicitationContext::new(signal), request);
        tokio::select! {
            result = answer => result.map_err(|error| ErrorData::internal_error(error.to_string(), None)),
            () = context.ct.cancelled() => {
                handle.cancel(CancelOptions::default());
                Err(ErrorData::internal_error("elicitation request cancelled", None))
            }
        }
    }

    async fn on_progress(
        &self,
        params: ProgressNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let Some(client) = self.client() else { return };
        let progress = McpProgress::new(params.progress, params.total, params.message);
        let callback = client
            .inner
            .progress
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .route(&params.progress_token, &progress);
        if let Some(callback) = callback {
            client.deliver_progress(&callback, progress);
        }
    }

    async fn on_logging_message(
        &self,
        params: LoggingMessageNotificationParam,
        _context: NotificationContext<RoleClient>,
    ) {
        let handler = self
            .client
            .upgrade()
            .and_then(|inner| inner.options.log_handler.clone());
        match handler {
            Some(handler) => handler(params),
            None => default_log_handler(&params),
        }
    }

    async fn on_tool_list_changed(&self, _context: NotificationContext<RoleClient>) {
        if let Some(client) = self.client() {
            client.tools_changed();
        }
    }
}

impl From<Arc<Inner>> for McpClient {
    fn from(inner: Arc<Inner>) -> Self {
        Self { inner }
    }
}
