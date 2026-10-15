//! Rust snippets for `observability-evaluation/logs.mdx`.

fn basic_subscriber_example() {
    // --8<-- [start:basic_subscriber]
    use tracing_subscriber::EnvFilter;

    // Show DEBUG and above for the SDK, WARN and above for everything else
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::new("warn,strands_agents=debug"))
        .init();
    // --8<-- [end:basic_subscriber]
}

fn env_filter_example() {
    // --8<-- [start:env_filter]
    use tracing_subscriber::EnvFilter;

    // Reads RUST_LOG, falling back to the SDK at DEBUG when it is unset
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("strands_agents=debug")),
        )
        .init();
    // --8<-- [end:env_filter]
}

fn module_filter_example() {
    // --8<-- [start:module_filter]
    use tracing_subscriber::EnvFilter;

    // DEBUG for the tool registry only, WARN for model interactions
    let filter = EnvFilter::new("warn")
        .add_directive("strands_agents::tools=debug".parse().unwrap())
        .add_directive("strands_agents::models=warn".parse().unwrap());

    tracing_subscriber::fmt().with_env_filter(filter).init();
    // --8<-- [end:module_filter]
}

fn json_subscriber_example() {
    // --8<-- [start:json_subscriber]
    use tracing_subscriber::EnvFilter;

    // One JSON object per event, for log shippers that parse structured output
    tracing_subscriber::fmt()
        .json()
        .with_current_span(true)
        .with_env_filter(EnvFilter::new("strands_agents=debug"))
        .init();
    // --8<-- [end:json_subscriber]
}
