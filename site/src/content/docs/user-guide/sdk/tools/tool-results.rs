//! Rust snippets for `tool-results.mdx`.

fn success_result_scope() {
    // --8<-- [start:success_result]
    use serde_json::json;
    use strands_agents::types::{ToolResultBlock, ToolResultContent};

    let result = ToolResultBlock::success(
        "tooluse_xq6vYsQ-QcGZOPcIx0yM3A",
        vec![
            ToolResultContent::text("Operation completed successfully"),
            ToolResultContent::Json(json!({ "results": [1, 2, 3], "total": 3 })),
        ],
    );
    // --8<-- [end:success_result]
}

fn error_result_scope() {
    // --8<-- [start:error_result]
    use strands_agents::types::ToolResultBlock;

    let result = ToolResultBlock::error(
        "tooluse_rFoPosVKQ7WfYRfw_min8Q",
        "Error: Unable to process request due to invalid parameters",
    );
    // --8<-- [end:error_result]
}
