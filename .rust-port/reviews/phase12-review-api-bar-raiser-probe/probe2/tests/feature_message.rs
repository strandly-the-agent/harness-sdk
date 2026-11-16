use strands_harness::*;

// The application enables `strands/anthropic` itself; `strands-harness` has no features on.
#[tokio::test]
async fn the_error_names_the_feature_to_enable() {
    let _direct = strands::models::AnthropicModel::builder(); // the strands feature is on
    let options = HarnessAgentOptions {
        model: Some("anthropic/claude-opus-5".into()),
        session: SessionOption::Off,
        memory: MemoryOption::Off,
        skills: SkillsOption::Off,
        ..Default::default()
    };
    let error = create_harness(options).await.err().unwrap();
    println!("{error}");
    assert!(!error.to_string().contains("requires the strands feature"), "{error}");
}
