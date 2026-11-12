//! The agent stream and hook events match the TS-written conformance fixtures.

use futures::StreamExt;
use serde_json::{Value, json};

use crate::agent::{Agent, AgentConfig};
use crate::models::Usage;
use crate::testing::{HookRecorder, MockModel, mock_tool, text_result};
use crate::types::{ContentBlock, ReasoningBlock, ToolUseBlock};

fn fixture(path: &str) -> Value {
    let full = format!("{}/../conformance/fixtures/{path}", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(full).unwrap()).unwrap()
}

/// Tracking ids and trace ids are random in Rust and counter-made in the fixtures.
fn scrub(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, value) in map.iter_mut() {
                if key == "trackingId" {
                    *value = json!("<id>");
                } else {
                    scrub(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(scrub),
        _ => {}
    }
}

fn loop_agent() -> Agent {
    let model = MockModel::new();
    model.add_turn_with(
        vec![
            ContentBlock::Reasoning(
                ReasoningBlock::new()
                    .with_text("I should look this up.")
                    .with_signature("reasoning-sig"),
            ),
            "Checking.".into(),
            ContentBlock::ToolUse(ToolUseBlock::new(
                "lookup",
                "tooluse-loop",
                json!({ "query": "weather" }),
            )),
        ],
        None,
        Some(Usage::new(11, 9, 20)),
    );
    model.add_turn_with(vec!["It is sunny.".into()], None, Some(Usage::new(25, 4, 29)));
    Agent::new(AgentConfig {
        name: Some("fixture-agent".into()),
        model: Some(model.into()),
        tools: crate::tool_list![mock_tool("lookup", |ctx| text_result(ctx, "sunny"))],
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn agent_stream_events_match_the_fixture() {
    let mut agent = loop_agent();
    let mut events = Vec::new();
    let mut stream = agent.stream("What is the weather?");
    while let Some(event) = stream.next().await {
        events.push(serde_json::to_value(event.unwrap()).unwrap());
    }
    let mut actual = Value::Array(events);
    let mut expected = fixture("stream/agent-stream-events.json");
    scrub(&mut actual);
    scrub(&mut expected);
    assert_eq!(actual, expected);
}

#[tokio::test]
async fn hook_events_match_the_fixture() {
    let mut agent = loop_agent();
    let recorder = HookRecorder::attach(agent.hooks());
    agent.invoke("What is the weather?").await.unwrap();
    let mut actual = Value::Array(recorder.events());
    let mut expected = fixture("hooks/hook-events.json");
    scrub(&mut actual);
    scrub(&mut expected);
    assert_eq!(actual, expected);
}

fn interrupt_agent() -> Agent {
    use crate::tools::{
        Tool, ToolOutcome, ToolResultStatus, ToolSpec, ToolStream, ToolStreamEvent, ToolStreamItem,
    };
    use crate::types::{InterruptParams, ToolResultBlock, ToolResultContent};

    struct Progress;
    impl Tool for Progress {
        fn name(&self) -> &str {
            "progress"
        }
        fn description(&self) -> &str {
            "Reports progress"
        }
        fn tool_spec(&self) -> ToolSpec {
            ToolSpec::new("progress", "Reports progress")
                .with_input_schema(json!({ "type": "object", "properties": {} }))
        }
        fn stream(&self, context: crate::ToolContext) -> ToolStream {
            let id = context.tool_use().tool_use_id.clone();
            ToolStream::new(futures::stream::iter([
                ToolStreamItem::Event(ToolStreamEvent::new(json!({ "step": 1, "of": 2 }))),
                ToolStreamItem::Outcome(ToolOutcome::Result(ToolResultBlock::new(
                    id,
                    ToolResultStatus::Success,
                    vec![ToolResultContent::from("progressed")],
                ))),
            ]))
        }
    }

    let model = MockModel::new();
    model.add_turn(vec![
        ContentBlock::ToolUse(ToolUseBlock::new("progress", "tooluse-progress", json!({}))),
        ContentBlock::ToolUse(ToolUseBlock::new("confirm", "tooluse-confirm", json!({}))),
    ]);
    model.add_turn(vec!["unused".into()]);
    let confirm = mock_tool("confirm", |ctx| {
        let params = InterruptParams {
            reason: Some(json!({ "prompt": "Continue?" })),
            ..InterruptParams::new("confirm_run")
        };
        match ctx.interrupt(params) {
            Ok(_) => text_result(ctx, "confirmed"),
            Err(interrupt) => ToolOutcome::Interrupt(interrupt),
        }
    });
    Agent::new(AgentConfig {
        name: Some("interrupt-stream-agent".into()),
        model: Some(model.into()),
        tools: crate::tool_list![Progress, confirm],
        printer: Some(false),
        ..Default::default()
    })
    .unwrap()
}

#[tokio::test]
async fn interrupt_stream_and_hook_events_match_the_fixtures() {
    let mut agent = interrupt_agent();
    let recorder = HookRecorder::attach(agent.hooks());
    let mut events = Vec::new();
    {
        let mut stream = agent.stream("Run with progress");
        while let Some(event) = stream.next().await {
            events.push(serde_json::to_value(event.unwrap()).unwrap());
        }
    }
    for (mut actual, path) in [
        (Value::Array(events), "stream/agent-stream-events-interrupt.json"),
        (
            Value::Array(recorder.events()),
            "hooks/hook-events-interrupt.json",
        ),
    ] {
        let mut expected = fixture(path);
        scrub(&mut actual);
        scrub(&mut expected);
        let types = |value: &Value| -> Vec<String> {
            value
                .as_array()
                .unwrap()
                .iter()
                .map(|e| e["type"].as_str().unwrap().to_owned())
                .collect()
        };
        assert_eq!(types(&actual), types(&expected), "{path}");
        for (a, e) in actual
            .as_array()
            .unwrap()
            .iter()
            .zip(expected.as_array().unwrap())
        {
            assert_eq!(a, e, "{path}");
        }
    }
}
