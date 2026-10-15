//! End-to-end decision runs through the public API, driven by `MockMessageModel`
//! (`strands-ts/src/decisions/__tests__/llm-decision-model.test.ts`).

use std::sync::Arc;

use serde_json::json;
use strands_agents::decisions::{
    Answer, DecisionModelExt, LlmDecisionModel, Question, Questions, Uncertainty,
};
use strands_agents::models::StreamOptions;
use strands_agents::test_support::{MockMessageModel, MockResponse};
use strands_agents::tools::STRUCTURED_OUTPUT_TOOL_NAME;
use strands_agents::types::{ContentBlock, ToolChoice, ToolUseBlock, Usage};
use strands_agents::{Message, Role};

/// An assistant turn that answers through the structured-output tool.
fn answers(input: serde_json::Value) -> Message {
    Message::new(
        Role::Assistant,
        vec![ContentBlock::ToolUse(ToolUseBlock {
            name: STRUCTURED_OUTPUT_TOOL_NAME.to_owned(),
            tool_use_id: "decision-1".to_owned(),
            input,
            reasoning_signature: None,
        })],
    )
}

const USAGE: Usage = Usage {
    input_tokens: 3,
    output_tokens: 2,
    total_tokens: 5,
    cache_read_input_tokens: None,
    cache_write_input_tokens: None,
};

/// The option enum the model is offered for `id`.
fn answer_enum(options: &StreamOptions, id: &str) -> serde_json::Value {
    let schema = options.tool_specs[0]
        .input_schema
        .as_ref()
        .expect("an input schema");
    schema["properties"][id]["properties"]["answer"]["enum"].clone()
}

fn questions() -> Questions {
    Questions::new()
        .boolean("is_fruit", "Is the thing talked about a fruit?")
        .choice("color", "What color is it?", ["red", "green", "blue"])
}

#[tokio::test]
async fn answers_boolean_and_choice_questions_in_one_call() {
    let model = MockMessageModel::shared(vec![MockResponse::with_usage(
        answers(json!({ "is_fruit": { "answer": "true" }, "color": { "answer": "red" } })),
        USAGE,
    )]);
    let decision = LlmDecisionModel::builder()
        .model(model.clone())
        .build()
        .await
        .expect("builds with an explicit model");

    let result = decision
        .ask("The fruit is an apple", &questions())
        .await
        .expect("decision succeeds");

    assert_eq!(result.answer("is_fruit").and_then(Answer::as_bool), Some(true));
    assert_eq!(result.answer("color").and_then(Answer::as_choice), Some("red"));
    assert_eq!(result.usage, Some(USAGE));
    assert_eq!(
        result.metadata.latency_ms, 0,
        "mock reports zero provider latency"
    );
    assert_eq!(model.call_count(), 1, "a decision is a single model call");
}

#[tokio::test]
async fn forces_the_structured_output_tool_and_the_hardened_system_prompt() {
    let model = MockMessageModel::shared(vec![MockResponse::message(answers(
        json!({ "is_fruit": { "answer": "false" }, "color": { "answer": "blue" } }),
    ))]);
    let decision = LlmDecisionModel::builder()
        .model(model.clone())
        .system_prompt("Judge the fruit.")
        .build()
        .await
        .expect("builds");

    decision
        .ask("a blueberry", &questions())
        .await
        .expect("decision succeeds");

    let (messages, options): (Vec<Message>, StreamOptions) = model.calls().remove(0);
    assert_eq!(messages.len(), 1, "a string state becomes one user message");
    assert_eq!(messages[0].role, Role::User);
    assert_eq!(
        messages[0].content,
        vec![ContentBlock::Text("a blueberry".to_owned())]
    );
    assert_eq!(
        options.tool_choice,
        Some(ToolChoice::Tool {
            name: STRUCTURED_OUTPUT_TOOL_NAME.to_owned()
        })
    );
    assert_eq!(options.tool_specs.len(), 1);
    assert_eq!(options.tool_specs[0].name, STRUCTURED_OUTPUT_TOOL_NAME);

    let prompt = options.system_prompt.expect("system prompt is sent").text();
    assert!(prompt.starts_with("Judge the fruit."), "{prompt}");
    assert!(prompt.contains("MANDATORY RULES"), "{prompt}");
    assert!(prompt.contains("untrusted data"), "{prompt}");
}

#[tokio::test]
async fn returns_uncertain_with_a_reason_when_the_model_opts_out() {
    let model = MockMessageModel::shared(vec![MockResponse::message(answers(json!({
        "color": { "answer": "uncertain", "reason": "the content never mentions a color" }
    })))]);
    let decision = LlmDecisionModel::new(model.clone());

    let result = decision
        .ask(
            "something edible",
            &Questions::new().choice("color", "What color is it?", ["red", "green"]),
        )
        .await
        .expect("decision succeeds");

    let uncertain = result
        .answer("color")
        .and_then(Answer::as_uncertain)
        .expect("uncertain answer");
    assert_eq!(
        uncertain.reason.as_deref(),
        Some("the content never mentions a color")
    );

    let options = model.calls().remove(0).1;
    assert_eq!(
        answer_enum(&options, "color"),
        json!(["red", "green", "uncertain"])
    );
}

#[tokio::test]
async fn opting_out_of_uncertainty_removes_the_option_from_the_schema() {
    let model = MockMessageModel::shared(vec![MockResponse::message(answers(
        json!({ "q": { "answer": "yes" } }),
    ))]);
    let decision = LlmDecisionModel::new(model.clone());

    decision
        .ask(
            "x",
            &Questions::new().with(
                "q",
                Question::choice("Sure?", ["yes", "no"]).with_uncertainty(Uncertainty::Disabled),
            ),
        )
        .await
        .expect("decision succeeds");

    let options = model.calls().remove(0).1;
    assert_eq!(answer_enum(&options, "q"), json!(["yes", "no"]));
}

#[tokio::test]
async fn falls_back_to_wall_clock_latency_when_the_provider_omits_metrics() {
    let model = MockMessageModel::shared(vec![MockResponse::message(answers(
        json!({ "q": { "answer": "true" } }),
    ))]);
    let decision = LlmDecisionModel::new(model);

    let result = decision
        .ask("x", &Questions::new().boolean("q", "ok?"))
        .await
        .expect("decision succeeds");

    assert!(result.usage.is_none(), "mock emitted no metadata event");
    assert!(result.metadata.latency_ms < 10_000);
}

#[tokio::test]
async fn an_invalid_question_set_fails_before_the_model_is_called() {
    let model = MockMessageModel::shared(vec![MockResponse::message(answers(json!({})))]);
    let decision = LlmDecisionModel::new(model.clone());

    let error = decision
        .ask("x", &Questions::new().choice("q", "pick", ["a", "a"]))
        .await
        .expect_err("duplicate choices are rejected");

    assert!(error.to_string().contains("choices must be unique"), "{error}");
    assert_eq!(model.call_count(), 0, "validation short-circuits the model call");
}

#[tokio::test]
async fn an_answer_outside_the_allowed_options_is_an_error() {
    let model = MockMessageModel::shared(vec![MockResponse::message(answers(
        json!({ "color": { "answer": "purple" } }),
    ))]);
    let decision = LlmDecisionModel::new(model);

    let error = decision
        .ask(
            "x",
            &Questions::new().choice("color", "What color?", ["red", "green"]),
        )
        .await
        .expect_err("an unknown option is rejected");

    assert!(error.to_string().contains("purple"), "{error}");
}

#[tokio::test]
async fn a_text_only_turn_is_an_error() {
    let model = MockMessageModel::shared(vec![MockResponse::message(Message::assistant("red, I think"))]);
    let decision = LlmDecisionModel::new(model);

    let error = decision
        .ask("x", &Questions::new().boolean("q", "ok?"))
        .await
        .expect_err("a non-tool-use stop is rejected");

    assert!(error.to_string().contains("no structured answers"), "{error}");
}

#[tokio::test]
async fn a_decision_model_is_usable_behind_a_trait_object() {
    let model = MockMessageModel::shared(vec![MockResponse::message(answers(
        json!({ "q": { "answer": "true" } }),
    ))]);
    let decision: Arc<dyn strands_agents::decisions::DecisionModel> = Arc::new(LlmDecisionModel::new(model));

    let result = decision
        .ask("x", &Questions::new().boolean("q", "ok?"))
        .await
        .expect("decision succeeds");

    assert_eq!(result.answer("q"), Some(&Answer::Bool(true)));
}

#[cfg(feature = "telemetry")]
mod tracing {
    //! `strands-ts/src/decisions/__tests__/llm-decision-model.tracing.test.node.ts`.

    use super::*;
    use opentelemetry::trace::{SpanId, Tracer as _};
    use opentelemetry_sdk::trace::{InMemorySpanExporter, SdkTracerProvider, SpanData};
    use strands_agents::models::{Model, ModelEventStream};

    /// Starts a span inside `stream`, the way a model provider would, then replays `inner`.
    struct ChildSpanModel {
        inner: Arc<MockMessageModel>,
    }

    impl Model for ChildSpanModel {
        fn model_id(&self) -> Option<String> {
            self.inner.model_id()
        }

        fn stream<'a>(&'a self, messages: &'a [Message], options: &'a StreamOptions) -> ModelEventStream<'a> {
            opentelemetry::global::tracer("test").in_span("child_in_decide", |_| ());
            self.inner.stream(messages, options)
        }
    }

    fn find<'a>(spans: &'a [SpanData], name: &str) -> &'a SpanData {
        spans.iter().find(|span| span.name == name).unwrap_or_else(|| {
            panic!(
                "no span named {name} in {:?}",
                spans.iter().map(|span| &span.name).collect::<Vec<_>>()
            )
        })
    }

    #[tokio::test]
    async fn the_decision_span_parents_child_spans_and_records_the_answers() {
        let exporter = InMemorySpanExporter::default();
        let provider = SdkTracerProvider::builder()
            .with_simple_exporter(exporter.clone())
            .build();
        opentelemetry::global::set_tracer_provider(provider.clone());

        let inner = MockMessageModel::shared(vec![MockResponse::with_usage(
            answers(json!({ "q": { "answer": "true" } })),
            USAGE,
        )]);
        let decision = LlmDecisionModel::new(ChildSpanModel { inner });

        decision
            .ask("x", &Questions::new().boolean("q", "ok?"))
            .await
            .expect("decision succeeds");
        provider.force_flush().expect("flush");

        let spans = exporter.get_finished_spans().expect("spans");
        let decision_span = find(&spans, "decision");
        let child = find(&spans, "child_in_decide");

        assert_eq!(
            decision_span.parent_span_id,
            SpanId::INVALID,
            "a decision runs with no parent span"
        );
        assert_eq!(child.parent_span_id, decision_span.span_context.span_id());

        let attr = |key: &str| {
            decision_span
                .attributes
                .iter()
                .find(|kv| kv.key.as_str() == key)
                .map(|kv| kv.value.to_string())
                .unwrap_or_default()
        };
        assert_eq!(attr("gen_ai.operation.name"), "decision");
        assert_eq!(attr("gen_ai.usage.input_tokens"), "3");
        assert_eq!(attr("gen_ai.decision.latency_ms"), "0");

        let events: Vec<&str> = decision_span.events.iter().map(|e| e.name.as_ref()).collect();
        assert!(events.contains(&"gen_ai.decision.questions"), "{events:?}");
        assert!(events.contains(&"gen_ai.decision.answers"), "{events:?}");

        provider.shutdown().expect("shutdown");
    }
}
