//! Ported from strands-ts/src/types/__tests__/agent.test.ts

use serde_json::json;

use super::*;
use crate::types::citations::{
    Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent, CitationsBlock,
};
use crate::types::messages::{CachePointBlock, ReasoningBlock, Role, ToolResultBlock, ToolUseBlock};
use crate::types::tools::ToolResultStatus;

fn assistant(content: Vec<ContentBlock>) -> Message {
    Message::new(Role::Assistant, content)
}

fn text_result(text: &str) -> AgentResult {
    AgentResult::new(StopReason::EndTurn, assistant(vec![text.into()])).with_metrics(AgentMetrics::new())
}

fn non_text_blocks() -> Vec<ContentBlock> {
    vec![
        ToolUseBlock::new("calc", "id-1", json!({ "a": 1, "b": 2 })).into(),
        ToolResultBlock::new("id-1", ToolResultStatus::Success, vec!["3".into()]).into(),
        CachePointBlock::new().into(),
    ]
}

fn result_json(result: &AgentResult) -> JsonValue {
    serde_json::to_value(result).unwrap()
}

fn traced(result: AgentResult, cycles: usize) -> AgentResult {
    let traces = (1..=cycles)
        .map(|cycle| AgentTrace::new(format!("Cycle {cycle}"), 0))
        .collect();
    result.with_traces(traces).with_metrics(AgentMetrics::new())
}

#[test]
fn when_content_is_empty_returns_empty_string() {
    let result = AgentResult::new(StopReason::EndTurn, assistant(vec![])).with_metrics(AgentMetrics::new());
    assert_eq!(result.to_string(), "");
}

#[test]
fn returns_the_text_content() {
    assert_eq!(text_result("Hello, world!").to_string(), "Hello, world!");
}

#[test]
fn when_content_has_multiple_text_blocks_returns_all_text_joined_with_newlines() {
    let message = assistant(vec![
        "First line".into(),
        "Second line".into(),
        "Third line".into(),
    ]);
    let result = AgentResult::new(StopReason::EndTurn, message);
    assert_eq!(result.to_string(), "First line\nSecond line\nThird line");
}

#[test]
fn returns_the_reasoning_text_with_prefix() {
    let message = assistant(vec![
        ReasoningBlock::new()
            .with_text("Let me think about this...")
            .into(),
    ]);
    let result = AgentResult::new(StopReason::EndTurn, message);
    assert_eq!(result.to_string(), "💭 Reasoning:\n   Let me think about this...");
}

#[test]
fn returns_empty_string_reasoning_block_is_skipped() {
    let message = assistant(vec![ReasoningBlock::new().with_signature("abc123").into()]);
    let result = AgentResult::new(StopReason::EndTurn, message);
    assert_eq!(result.to_string(), "");
}

#[test]
fn when_content_has_mixed_text_block_and_reasoning_block_returns_all_text_joined_with_newlines() {
    let message = assistant(vec![
        "Here is my response.".into(),
        ReasoningBlock::new().with_text("I reasoned carefully.").into(),
        "Additional context.".into(),
    ]);
    let result = AgentResult::new(StopReason::EndTurn, message);
    assert_eq!(
        result.to_string(),
        "Here is my response.\n💭 Reasoning:\n   I reasoned carefully.\nAdditional context."
    );
}

#[test]
fn reasoning_text_lines_are_indented() {
    let message = assistant(vec![ReasoningBlock::new().with_text("one\ntwo").into()]);
    let result = AgentResult::new(StopReason::EndTurn, message);
    assert_eq!(result.to_string(), "💭 Reasoning:\n   one\n   two");
}

#[test]
fn when_content_has_only_non_text_blocks_returns_empty_string() {
    let result = AgentResult::new(StopReason::ToolUse, assistant(non_text_blocks()));
    assert_eq!(result.to_string(), "");
}

#[test]
fn returns_only_text_from_text_block_and_reasoning_block() {
    let message = assistant(vec![
        "Before tool".into(),
        ToolUseBlock::new("calc", "id-1", json!({ "a": 1, "b": 2 })).into(),
        ReasoningBlock::new().with_text("Thinking...").into(),
        CachePointBlock::new().into(),
        "After tool".into(),
    ]);
    let result = AgentResult::new(StopReason::ToolUse, message);
    assert_eq!(
        result.to_string(),
        "Before tool\n💭 Reasoning:\n   Thinking...\nAfter tool"
    );
}

#[test]
fn returns_json_stringified_interrupts_taking_priority_over_text_content() {
    let interrupt = Interrupt::new("i-1", "confirm").with_reason(json!("ok?"));
    let result = AgentResult::new(StopReason::Interrupt, assistant(vec!["ignored".into()]))
        .with_interrupts(vec![interrupt]);
    assert_eq!(
        result.to_string(),
        r#"[{"id":"i-1","name":"confirm","reason":"ok?","source":"hook"}]"#
    );
}

#[test]
fn falls_through_when_interrupts_array_is_empty() {
    let result =
        AgentResult::new(StopReason::EndTurn, assistant(vec!["Hello".into()])).with_interrupts(vec![]);
    assert_eq!(result.to_string(), "Hello");
}

#[test]
fn returns_json_stringified_structured_output_taking_priority_over_text_content() {
    let result = AgentResult::new(StopReason::EndTurn, assistant(vec!["ignored".into()]))
        .with_structured_output(json!({ "answer": 42, "note": "hello" }));
    assert_eq!(result.to_string(), r#"{"answer":42,"note":"hello"}"#);
}

#[test]
fn returns_interrupts_taking_priority_over_structured_output() {
    let result = AgentResult::new(StopReason::Interrupt, assistant(vec!["ignored".into()]))
        .with_interrupts(vec![Interrupt::new("i-1", "confirm")])
        .with_structured_output(json!({ "answer": 42 }));
    assert_eq!(
        result.to_string(),
        r#"[{"id":"i-1","name":"confirm","source":"hook"}]"#
    );
}

#[test]
fn concatenates_generated_content_text_from_citations() {
    let citation = Citation::new(
        CitationLocation::document_char(0, 0, 5),
        "doc",
        vec![CitationSourceContent::new("source text")],
        "Doc",
    );
    let citations = CitationsBlock::new(
        vec![citation],
        vec![CitationGeneratedContent::new("cited fragment")],
    );
    let message = assistant(vec!["Here is a citation:".into(), citations.into()]);
    let result = AgentResult::new(StopReason::EndTurn, message);
    assert_eq!(result.to_string(), "Here is a citation:\ncited fragment");
}

#[test]
fn works_with_string_conversion() {
    assert_eq!(text_result("Hello").to_string(), "Hello");
}

#[test]
fn works_with_template_literals() {
    assert_eq!(format!("Response: {}", text_result("World")), "Response: World");
}

#[test]
fn returns_latest_context_size_from_metrics() {
    let result = text_result("Hello").with_metrics(AgentMetrics::new().with_latest_context_size(500));
    assert_eq!(result.context_size(), Some(500));
}

#[test]
fn returns_undefined_when_metrics_has_no_latest_context_size() {
    assert_eq!(text_result("Hello").context_size(), None);
}

#[test]
fn returns_undefined_when_no_metrics_are_available() {
    let result = AgentResult::new(StopReason::EndTurn, assistant(vec!["Hello".into()]));
    assert_eq!(result.context_size(), None);
}

#[test]
fn returns_projected_context_size_from_metrics() {
    let result = text_result("Hello").with_metrics(AgentMetrics::new().with_projected_context_size(750));
    assert_eq!(result.projected_context_size(), Some(750));
}

#[test]
fn returns_undefined_when_metrics_has_no_projected_context_size() {
    assert_eq!(text_result("Hello").projected_context_size(), None);
}

#[test]
fn excludes_traces_and_metrics_from_serialization() {
    let message = assistant(vec!["Hello".into()]);
    let result = traced(AgentResult::new(StopReason::EndTurn, message.clone()), 1);
    assert_eq!(
        result_json(&result),
        json!({ "type": "agentResult", "stopReason": "endTurn", "lastMessage": message })
    );
}

#[test]
fn includes_structured_output_when_present() {
    let result = AgentResult::new(StopReason::EndTurn, assistant(vec!["Response".into()]))
        .with_structured_output(json!({ "field": "value" }));
    assert_eq!(
        result_json(&result)["structuredOutput"],
        json!({ "field": "value" })
    );
}

#[test]
fn excludes_structured_output_when_not_present() {
    let result = AgentResult::new(StopReason::EndTurn, assistant(vec!["Response".into()]));
    assert!(result_json(&result).get("structuredOutput").is_none());
}

#[test]
fn is_automatically_used_by_json_stringify() {
    let message = assistant(vec!["Hello".into()]);
    let result = traced(AgentResult::new(StopReason::EndTurn, message.clone()), 1);
    let parsed: JsonValue = serde_json::from_str(&json::to_string(&result).unwrap()).unwrap();
    assert_eq!(
        parsed,
        json!({ "type": "agentResult", "stopReason": "endTurn", "lastMessage": message })
    );
}

#[test]
fn preserves_traces_and_metrics_as_accessible_properties() {
    let message = assistant(vec!["Hello".into()]);
    let traces = vec![AgentTrace::new("Cycle 1", 0)];
    let result = AgentResult::new(StopReason::EndTurn, message.clone())
        .with_traces(traces.clone())
        .with_metrics(AgentMetrics::new());
    assert_eq!(result.traces(), traces.as_slice());
    assert_eq!(result.metrics(), Some(&AgentMetrics::new()));
    assert_eq!(
        result_json(&result),
        json!({ "type": "agentResult", "stopReason": "endTurn", "lastMessage": message })
    );
}

#[test]
fn prevents_bloated_api_responses_when_forwarding_result_directly() {
    let message = assistant(vec!["Response text".into()]);
    let result = traced(AgentResult::new(StopReason::EndTurn, message.clone()), 3);
    let response = result_json(&result);
    assert_eq!(
        response,
        json!({ "type": "agentResult", "stopReason": "endTurn", "lastMessage": message })
    );
    assert!(response.get("traces").is_none());
    assert!(response.get("metrics").is_none());
}

#[test]
fn omits_checkpoint_from_to_json_when_absent() {
    assert!(result_json(&text_result("hi")).get("checkpoint").is_none());
}

#[test]
fn structured_output_deserializes_the_validated_output() {
    #[derive(Debug, PartialEq, serde::Deserialize)]
    struct Weather {
        city: String,
    }
    let result = text_result("ignored").with_structured_output(json!({ "city": "Paris" }));
    assert_eq!(
        result.structured_output::<Weather>().unwrap(),
        Weather { city: "Paris".into() }
    );
    assert!(matches!(
        result.structured_output::<u32>(),
        Err(crate::Error::Other(_))
    ));
    let missing = text_result("ignored").structured_output::<Weather>().unwrap_err();
    assert_eq!(missing.to_string(), "the result has no structured output");
}

#[test]
fn a_saved_result_loads_with_empty_live_fields() {
    let result: AgentResult = serde_json::from_value(json!({
        "type": "agentResult",
        "stopReason": "endTurn",
        "lastMessage": { "role": "assistant", "content": [{ "text": "Hi" }], "trackingId": "t-1" },
        "structuredOutput": null
    }))
    .unwrap();
    assert_eq!(result.stop_reason(), StopReason::EndTurn);
    assert_eq!(result.structured_output::<JsonValue>().unwrap(), JsonValue::Null);
    assert!(result.metrics().is_none() && result.traces().is_empty() && result.interrupts().is_empty());
    assert!(serde_json::from_value::<AgentResult>(json!({ "type": "other" })).is_err());
}

#[test]
fn equality_ignores_the_invocation_state() {
    let state = InvocationState::default();
    state.insert(7_u32);
    let result = text_result("Hi");
    assert_eq!(result.clone().with_invocation_state(state), result);
}

#[test]
fn agent_metrics_serialize_in_the_ts_to_json_order() {
    let invocation = InvocationMetricsData::new(
        vec![AgentLoopMetricsData::new("cycle-1", 5, Usage::new(1, 2, 3))],
        Usage::new(1, 2, 3),
    );
    let metrics = AgentMetrics::new()
        .with_cycle_count(1)
        .with_accumulated_usage(Usage::new(1, 2, 3))
        .with_accumulated_metrics(Metrics::new(9))
        .with_agent_invocations(vec![invocation.clone()])
        .with_tool_metric("calc", ToolMetricsData::new(2, 1, 1, 30))
        .with_total_duration(5)
        .with_latest_context_size(1)
        .with_projected_context_size(3);
    let written = json::to_string(&metrics).unwrap();
    assert_eq!(
        written,
        concat!(
            r#"{"cycleCount":1,"accumulatedUsage":{"inputTokens":1,"outputTokens":2,"totalTokens":3},"#,
            r#""accumulatedMetrics":{"latencyMs":9},"agentInvocations":[{"cycles":[{"cycleId":"cycle-1","#,
            r#""duration":5,"usage":{"inputTokens":1,"outputTokens":2,"totalTokens":3}}],"usage":"#,
            r#"{"inputTokens":1,"outputTokens":2,"totalTokens":3}}],"toolMetrics":{"calc":{"callCount":2,"#,
            r#""successCount":1,"errorCount":1,"totalTime":30}},"totalDuration":5,"latestContextSize":1,"#,
            r#""projectedContextSize":3}"#
        )
    );
    assert_eq!(serde_json::from_str::<AgentMetrics>(&written).unwrap(), metrics);
    assert_eq!(metrics.latest_agent_invocation(), Some(&invocation));
    assert_eq!(metrics.average_cycle_time(), 5.0);
    assert_eq!(
        metrics.tool_metrics().collect::<Vec<_>>(),
        [("calc", &ToolMetricsData::new(2, 1, 1, 30))]
    );
    assert_eq!(metrics.tool_metric("calc").unwrap().average_time(), 15.0);
}

#[test]
fn empty_agent_metrics_match_the_ts_defaults() {
    assert_eq!(
        serde_json::to_value(AgentMetrics::new()).unwrap(),
        json!({
            "cycleCount": 0,
            "accumulatedUsage": { "inputTokens": 0, "outputTokens": 0, "totalTokens": 0 },
            "accumulatedMetrics": { "latencyMs": 0 },
            "agentInvocations": [],
            "toolMetrics": {},
            "totalDuration": 0
        })
    );
    assert_eq!(
        serde_json::from_value::<AgentMetrics>(json!({})).unwrap(),
        AgentMetrics::new()
    );
    assert_eq!(AgentMetrics::new().average_cycle_time(), 0.0);
    assert_eq!(ToolMetricsData::default().success_rate(), 0.0);
}

#[test]
fn agent_trace_serializes_in_the_ts_to_json_shape() {
    let mut trace = AgentTrace::new("Cycle 1", 100).with_id("root").with_child(
        AgentTrace::new("Tool: calc", 110)
            .with_id("child")
            .with_metadata("toolName", "calc"),
    );
    trace.end(150);
    assert_eq!(
        serde_json::to_value(&trace).unwrap(),
        json!({
            "id": "root",
            "name": "Cycle 1",
            "parentId": null,
            "startTime": 100,
            "endTime": 150,
            "duration": 50,
            "children": [{
                "id": "child",
                "name": "Tool: calc",
                "parentId": "root",
                "startTime": 110,
                "endTime": null,
                "duration": 0,
                "children": [],
                "metadata": { "toolName": "calc" },
                "message": null
            }],
            "metadata": {},
            "message": null
        })
    );
    let round_trip: AgentTrace = serde_json::from_value(serde_json::to_value(&trace).unwrap()).unwrap();
    assert_eq!(round_trip, trace);
    assert_eq!(
        round_trip.children()[0].metadata().collect::<Vec<_>>(),
        [("toolName", "calc")]
    );
}

#[test]
fn agent_trace_ids_are_unique_uuids() {
    let first = AgentTrace::new("a", 0);
    let second = AgentTrace::new("b", 0);
    assert_ne!(first.id(), second.id());
    assert!(uuid::Uuid::parse_str(first.id()).is_ok());
}

#[test]
fn agent_result_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>(_: &T) {}
    assert_send_sync(&text_result("Hi"));
}
