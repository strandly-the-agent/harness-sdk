//! MCP protocol and SDK errors (the `@modelcontextprotocol/client` `ProtocolError` and
//! `SdkError` the TS client surfaces).

use rmcp::ServiceError;
use rmcp::service::ClientInitializeError;

use crate::types::json::JsonValue;

/// TS `ProtocolErrorCode.UrlElicitationRequired`.
pub(crate) const URL_ELICITATION_REQUIRED: i32 = -32042;

/// Which MCP SDK error class an [`McpError`] stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum McpErrorKind {
    /// A JSON-RPC error the server returned (TS `ProtocolError`), with its code.
    Protocol {
        /// The JSON-RPC error code.
        code: i32,
    },
    /// A client-side failure: transport, timeout, closed connection, pagination cap (TS
    /// `SdkError`).
    Sdk,
}

/// An MCP request failed (TS `ProtocolError` or `SdkError`).
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{message}")]
#[non_exhaustive]
pub struct McpError {
    kind: McpErrorKind,
    message: String,
    data: Option<JsonValue>,
}

impl McpError {
    /// A JSON-RPC error from the server.
    pub fn protocol(code: i32, message: impl Into<String>, data: Option<JsonValue>) -> Self {
        Self {
            kind: McpErrorKind::Protocol { code },
            message: message.into(),
            data,
        }
    }

    /// A client-side failure.
    pub fn sdk(message: impl Into<String>) -> Self {
        Self {
            kind: McpErrorKind::Sdk,
            message: message.into(),
            data: None,
        }
    }

    /// Which error class this is.
    pub fn kind(&self) -> McpErrorKind {
        self.kind
    }

    /// The JSON-RPC code of a protocol error.
    pub fn code(&self) -> Option<i32> {
        match self.kind {
            McpErrorKind::Protocol { code } => Some(code),
            McpErrorKind::Sdk => None,
        }
    }

    /// The message.
    pub fn message(&self) -> &str {
        &self.message
    }

    /// The structured error data.
    pub fn data(&self) -> Option<&JsonValue> {
        self.data.as_ref()
    }

    /// The TS `error.name`: `ProtocolError` or `SdkError`.
    pub fn name(&self) -> &'static str {
        match self.kind {
            McpErrorKind::Protocol { .. } => "ProtocolError",
            McpErrorKind::Sdk => "SdkError",
        }
    }

    /// TS `SdkError(ConnectionClosed, 'MCP client disconnected')`.
    pub(crate) fn disconnected() -> Self {
        Self::sdk("MCP client disconnected")
    }
}

impl From<ServiceError> for McpError {
    fn from(error: ServiceError) -> Self {
        match error {
            ServiceError::McpError(data) => Self::protocol(data.code.0, data.message, data.data),
            ServiceError::Timeout { .. } => Self::sdk("Request timed out"),
            ServiceError::TransportClosed => Self::sdk("Connection closed"),
            other => Self::sdk(other.to_string()),
        }
    }
}

impl From<ClientInitializeError> for McpError {
    fn from(error: ClientInitializeError) -> Self {
        match error {
            ClientInitializeError::JsonRpcError(data) => Self::protocol(data.code.0, data.message, data.data),
            other => Self::sdk(other.to_string()),
        }
    }
}

impl From<ServiceError> for crate::Error {
    fn from(error: ServiceError) -> Self {
        Self::Mcp(error.into())
    }
}
