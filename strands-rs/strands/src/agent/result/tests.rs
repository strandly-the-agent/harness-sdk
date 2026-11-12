//! Ported from strands-ts/src/types/__tests__/agent.test.ts
//!
//! The `checkpoint` cases are out of scope (checkpointing is out of v0.1). `AgentResult` always
//! carries metrics, so "no metrics" is the default `AgentMetrics`.

use serde_json::json;

use super::*;
use crate::agent::InvocationState;
use crate::tools::ToolResultStatus;
use crate::types::{
    CachePointBlock, Citation, CitationGeneratedContent, CitationLocation, CitationSourceContent,
    CitationsBlock, ReasoningBlock, Role, TextBlock, ToolResultBlock, ToolResultContent, ToolUseBlock,
};

fn result(content: Vec<ContentBlock>) -> AgentResult {
    AgentResult::new(
        StopReason::EndTurn,
        Message::new(Role::Assistant, content),
        InvocationState::default(),
    )
}

fn text(text: &str) -> ContentBlock {
    TextBlock::new(text).into()
}

fn reasoning(text: &str) -> ContentBlock {
    ReasoningBlock::new().with_text(text).into()
}

fn tool_use() -> ContentBlock {
    ToolUseBlock::new("calc", "id-1", json!({"a": 1, "b": 2})).into()
}

mod to_string {
    use super::*;

    #[test]
    fn returns_empty_string_when_content_is_empty() {
        assert_eq!(result(vec![]).to_string(), "");
    }

    #[test]
    fn returns_the_text_content_of_a_single_text_block() {
        assert_eq!(result(vec![text("Hello, world!")]).to_string(), "Hello, world!");
    }

    #[test]
    fn returns_all_text_joined_with_newlines() {
        let result = result(vec![text("First line"), text("Second line"), text("Third line")]);
        assert_eq!(result.to_string(), "First line\nSecond line\nThird line");
    }

    #[test]
    fn returns_the_reasoning_text_with_prefix() {
        let result = result(vec![reasoning("Let me think about this...")]);
        assert_eq!(result.to_string(), "💭 Reasoning:\n   Let me think about this...");
    }

    #[test]
    fn skips_a_reasoning_block_without_text() {
        let result = result(vec![ReasoningBlock::new().with_signature("abc123").into()]);
        assert_eq!(result.to_string(), "");
    }

    #[test]
    fn joins_mixed_text_and_reasoning_blocks() {
        let result = result(vec![
            text("Here is my response."),
            reasoning("I reasoned carefully."),
            text("Additional context."),
        ]);
        assert_eq!(
            result.to_string(),
            "Here is my response.\n💭 Reasoning:\n   I reasoned carefully.\nAdditional context."
        );
    }

    #[test]
    fn returns_empty_string_for_only_non_text_blocks() {
        let result = result(vec![
            tool_use(),
            ToolResultBlock::new(
                "id-1",
                ToolResultStatus::Success,
                vec![ToolResultContent::Text("3".into())],
            )
            .into(),
            CachePointBlock::new().into(),
        ]);
        assert_eq!(result.to_string(), "");
    }

    #[test]
    fn returns_only_text_from_text_and_reasoning_blocks() {
        let result = result(vec![
            text("Before tool"),
            tool_use(),
            reasoning("Thinking..."),
            CachePointBlock::new().into(),
            text("After tool"),
        ]);
        assert_eq!(
            result.to_string(),
            "Before tool\n💭 Reasoning:\n   Thinking...\nAfter tool"
        );
    }

    #[test]
    fn returns_json_interrupts_taking_priority_over_text_content() {
        let interrupt = Interrupt::new("i-1", "confirm").with_reason(json!("ok?"));
        let result = result(vec![text("ignored")]).with_interrupts(vec![interrupt.clone()]);
        assert_eq!(result.to_string(), serde_json::to_string(&[interrupt]).unwrap());
    }

    #[test]
    fn falls_through_when_interrupts_are_empty() {
        let result = result(vec![text("Hello")]).with_interrupts(vec![]);
        assert_eq!(result.to_string(), "Hello");
    }

    #[test]
    fn returns_json_structured_output_taking_priority_over_text_content() {
        let output = json!({"answer": 42, "note": "hello"});
        let result = result(vec![text("ignored")]).with_structured_output(Some(output.clone()));
        assert_eq!(result.to_string(), serde_json::to_string(&output).unwrap());
    }

    #[test]
    fn returns_interrupts_taking_priority_over_structured_output() {
        let interrupt = Interrupt::new("i-1", "confirm");
        let result = result(vec![text("ignored")])
            .with_interrupts(vec![interrupt.clone()])
            .with_structured_output(Some(json!({"answer": 42})));
        assert_eq!(result.to_string(), serde_json::to_string(&[interrupt]).unwrap());
    }

    #[test]
    fn concatenates_generated_content_text_from_citations() {
        let citation = Citation::new(
            CitationLocation::DocumentChar {
                document_index: 0,
                start: 0,
                end: 5,
            },
            "doc",
            vec![CitationSourceContent::new("source text")],
            "Doc",
        );
        let citations = CitationsBlock::new(
            vec![citation],
            vec![CitationGeneratedContent::new("cited fragment")],
        );
        let result = result(vec![text("Here is a citation:"), citations.into()]);
        assert_eq!(result.to_string(), "Here is a citation:\ncited fragment");
    }

    #[test]
    fn works_with_format_interpolation() {
        assert_eq!(
            format!("Response: {}", result(vec![text("World")])),
            "Response: World"
        );
    }
}

mod context_size {
    use super::*;

    fn with_metrics(metrics: AgentMetrics) -> AgentResult {
        result(vec![text("Hello")]).with_metrics(metrics)
    }

    #[test]
    fn returns_latest_context_size_from_metrics() {
        let metrics = AgentMetrics {
            latest_context_size: Some(500),
            ..AgentMetrics::default()
        };
        assert_eq!(with_metrics(metrics).context_size(), Some(500));
    }

    #[test]
    fn returns_none_when_metrics_have_no_latest_context_size() {
        assert_eq!(with_metrics(AgentMetrics::default()).context_size(), None);
        assert_eq!(result(vec![text("Hello")]).context_size(), None);
    }

    #[test]
    fn returns_projected_context_size_from_metrics() {
        let metrics = AgentMetrics {
            projected_context_size: Some(750),
            ..AgentMetrics::default()
        };
        assert_eq!(with_metrics(metrics).projected_context_size(), Some(750));
    }

    #[test]
    fn returns_none_when_metrics_have_no_projected_context_size() {
        assert_eq!(
            with_metrics(AgentMetrics::default()).projected_context_size(),
            None
        );
    }
}

mod to_json {
    use super::*;

    fn traced(names: &[&str]) -> AgentResult {
        let traces = names.iter().map(|name| AgentTrace::new(*name, None, 0)).collect();
        result(vec![text("Hello")])
            .with_traces(traces)
            .with_metrics(AgentMetrics::default())
    }

    #[test]
    fn excludes_traces_and_metrics_from_serialization() {
        let result = traced(&["Cycle 1"]);
        assert_eq!(
            serde_json::to_value(&result).unwrap(),
            json!({
                "type": "agentResult",
                "stopReason": "endTurn",
                "lastMessage": serde_json::to_value(result.last_message()).unwrap(),
            })
        );
    }

    #[test]
    fn includes_structured_output_when_present() {
        let output = json!({"field": "value"});
        let result = result(vec![text("Response")]).with_structured_output(Some(output.clone()));
        assert_eq!(serde_json::to_value(&result).unwrap()["structuredOutput"], output);
    }

    #[test]
    fn excludes_structured_output_when_not_present() {
        let json = serde_json::to_value(result(vec![text("Response")])).unwrap();
        assert!(json.get("structuredOutput").is_none());
    }

    #[test]
    fn serializes_to_a_string_with_the_last_message_content() {
        let parsed: JsonValue =
            serde_json::from_str(&serde_json::to_string(&traced(&["Cycle 1"])).unwrap()).unwrap();
        assert_eq!(parsed["type"], "agentResult");
        assert_eq!(parsed["stopReason"], "endTurn");
        assert!(parsed["lastMessage"]["content"].is_array());
    }

    #[test]
    fn keeps_traces_and_metrics_accessible_but_out_of_json() {
        let result = traced(&["Cycle 1", "Cycle 2", "Cycle 3"]);
        let names: Vec<&str> = result.traces().iter().map(|trace| trace.name.as_str()).collect();
        assert_eq!(names, ["Cycle 1", "Cycle 2", "Cycle 3"]);
        assert_eq!(result.metrics().cycle_count, 0);
        let json = serde_json::to_value(&result).unwrap();
        assert!(json.get("traces").is_none());
        assert!(json.get("metrics").is_none());
        assert_eq!(json.as_object().unwrap().len(), 3);
    }
}
