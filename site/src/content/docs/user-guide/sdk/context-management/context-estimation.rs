//! Rust snippets for `context-management/context-estimation.mdx`.

use strands_agents::Result;

async fn estimation() -> Result<()> {
    // --8<-- [start:context_window_limit]
    use strands_agents::models::{BedrockModel, BedrockModelConfig};

    let model = BedrockModel::new(BedrockModelConfig {
        model_id: Some("my-custom-model".to_owned()),
        context_window_limit: Some(128_000),
        ..Default::default()
    })
    .await?;
    // --8<-- [end:context_window_limit]

    let projected_tokens = 50_000;
    // --8<-- [start:utilization]
    use strands_agents::Model;

    let ratio = model.estimate_utilization(projected_tokens);
    // ratio is 0-1+ (above 1.0 means overflow)
    // --8<-- [end:utilization]
    let _ = ratio;
    Ok(())
}
