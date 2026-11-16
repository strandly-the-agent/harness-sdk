use strands::indexmap::IndexMap;
use strands::mcp::{McpClientCredentials, McpServerConfig};
use strands_harness::{HarnessAgentOptions, McpServersOption};

#[test]
fn debug_of_options_does_not_print_the_mcp_client_secret() {
    let credentials = McpClientCredentials {
        client_id: "id".into(),
        client_secret: "s3cr3t-value".into(),
        scopes: None,
    };
    // The SDK's own Debug redacts it.
    assert!(!format!("{credentials:?}").contains("s3cr3t-value"));
    let server = McpServerConfig {
        url: Some("https://example.com/mcp".into()),
        auth: Some(credentials),
        ..Default::default()
    };
    let servers: IndexMap<String, McpServerConfig> = [("remote".to_owned(), server)].into_iter().collect();
    let options = HarnessAgentOptions::builder().mcp_servers(McpServersOption::from(servers)).build();
    let printed = format!("{options:?}");
    assert!(!printed.contains("s3cr3t-value"), "{printed}");
}
