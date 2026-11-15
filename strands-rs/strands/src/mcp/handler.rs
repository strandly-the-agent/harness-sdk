//! The `rmcp` client handler: identity and capabilities, and the server-to-client requests and
//! notifications the TS client wires through its SDK `Client`.

use std::sync::Weak;

use rmcp::ClientHandler;
use rmcp::model::{ClientCapabilities, ClientConfig, Implementation};

use super::client::Inner;

/// Routes server traffic to the owning client; holds it weakly so the connection doesn't keep the
/// client alive.
pub(crate) struct McpHandler {
    #[expect(dead_code, reason = "read by the server-request handlers added with item 3")]
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
}

impl ClientHandler for McpHandler {
    fn get_info(&self) -> ClientConfig {
        self.info.clone()
    }
}
