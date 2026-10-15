//! The [`DecisionModel`] trait and its traced `ask` wrapper
//! (`strands-ts/src/decisions/decision-model.ts`).

use async_trait::async_trait;

use super::types::{DecisionInput, DecisionResult, Question, Questions, Uncertainty};
use crate::error::{Error, Result};
use crate::telemetry::{TelemetryConfig, Tracer};

/// Anything that can answer closed questions about some state.
///
/// Implement [`decide`](Self::decide); callers use [`ask`](DecisionModelExt::ask), which validates
/// the questions and traces the call. Object-safe: `Arc<dyn DecisionModel>`.
#[async_trait]
pub trait DecisionModel: Send + Sync {
    /// Implementation-specific decision logic (TS `_ask`).
    ///
    /// Must return a complete [`DecisionResult`], including `metadata.latency_ms` sourced from the
    /// underlying model provider. `questions` is already validated.
    async fn decide(&self, state: DecisionInput, questions: &Questions) -> Result<DecisionResult>;
}

/// The public, traced entry point for every [`DecisionModel`].
///
/// Blanket-implemented, so it also applies to `dyn DecisionModel`; `ask` cannot be overridden and
/// therefore every implementation is validated and traced identically.
#[async_trait]
pub trait DecisionModelExt {
    /// Ask one or more closed questions about `state`.
    ///
    /// Opens a `decision` span, validates `questions`, delegates to
    /// [`DecisionModel::decide`], and records usage, latency and the answer set on the span.
    ///
    /// # Errors
    ///
    /// [`Error::Other`] if `questions` is empty or holds an invalid question text, choice list or
    /// uncertainty configuration; otherwise whatever `decide` returns.
    async fn ask(
        &self,
        state: impl Into<DecisionInput> + Send,
        questions: &Questions,
    ) -> Result<DecisionResult>;
}

#[async_trait]
impl<T: DecisionModel + ?Sized> DecisionModelExt for T {
    async fn ask(
        &self,
        state: impl Into<DecisionInput> + Send,
        questions: &Questions,
    ) -> Result<DecisionResult> {
        validate_questions(questions)?;

        let tracer = Tracer::new(TelemetryConfig::from_env());
        let span = tracer.start_decision_span(&questions.ids().collect::<Vec<_>>());

        match crate::telemetry::in_span(&span, self.decide(state.into(), questions)).await {
            Ok(result) => {
                tracer.end_decision_span(
                    &span,
                    Some(result.metadata.latency_ms),
                    result.usage.as_ref(),
                    Some(&serde_json::json!(result.answers)),
                    None,
                );
                Ok(result)
            }
            Err(error) => {
                tracer.end_decision_span(&span, None, None, None, Some(&error.to_string()));
                Err(error)
            }
        }
    }
}

/// Reserved choice value that signals uncertainty on the wire.
pub(crate) const UNCERTAIN_LITERAL: &str = "uncertain";

fn validate_questions(questions: &Questions) -> Result<()> {
    if questions.is_empty() {
        return Err(Error::other("ask requires at least one question"));
    }

    for (id, question) in questions.iter() {
        let invalid = |message: &str| Err(Error::other(format!("question id=<{id}> | {message}")));

        if question.text().trim().is_empty() {
            return invalid("question must be a non-empty string");
        }
        if let Question::Choice { choices, .. } = question {
            if choices.is_empty() {
                return invalid("choices must be 'boolean' or a non-empty string array");
            }
            let unique: std::collections::HashSet<&String> = choices.iter().collect();
            if unique.len() != choices.len() {
                return invalid("choices must be unique");
            }
            if choices.iter().any(String::is_empty) {
                return invalid("choices must be non-empty strings");
            }
            if choices.iter().any(|choice| choice == UNCERTAIN_LITERAL) {
                return invalid("choice 'uncertain' is reserved for the Uncertain answer");
            }
        }
        if let Some(Uncertainty::MinConfidence(min)) = question.uncertainty() {
            if !min.is_finite() || min < 0.0 || min > 1.0 {
                return invalid("uncertainty.minConfidence must be in [0.0, 1.0]");
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decisions::types::Answer;
    use indexmap::IndexMap;

    /// A model that records what it was asked and answers everything `true`.
    #[derive(Default)]
    struct RecordingDecisionModel {
        calls: parking_lot::Mutex<Vec<(DecisionInput, Questions)>>,
    }

    #[async_trait]
    impl DecisionModel for RecordingDecisionModel {
        async fn decide(&self, state: DecisionInput, questions: &Questions) -> Result<DecisionResult> {
            self.calls.lock().push((state, questions.clone()));
            let answers: IndexMap<String, Answer> = questions
                .ids()
                .map(|id| (id.to_owned(), Answer::Bool(true)))
                .collect();
            Ok(DecisionResult::new(answers, 1))
        }
    }

    fn error_of(result: Result<DecisionResult>) -> String {
        result.err().expect("must fail").to_string()
    }

    #[tokio::test]
    async fn ask_validates_before_delegating() {
        let model = RecordingDecisionModel::default();

        assert_eq!(
            error_of(model.ask("x", &Questions::new()).await),
            "ask requires at least one question"
        );
        assert_eq!(
            error_of(model.ask("x", &Questions::new().boolean("q", "   ")).await),
            "question id=<q> | question must be a non-empty string"
        );
        assert_eq!(
            error_of(
                model
                    .ask("x", &Questions::new().choice("q", "pick", Vec::<String>::new()))
                    .await
            ),
            "question id=<q> | choices must be 'boolean' or a non-empty string array"
        );
        assert_eq!(
            error_of(model.ask("x", &Questions::new().choice("q", "pick", ["a", "a"])).await),
            "question id=<q> | choices must be unique"
        );
        assert_eq!(
            error_of(model.ask("x", &Questions::new().choice("q", "pick", ["a", ""])).await),
            "question id=<q> | choices must be non-empty strings"
        );
        assert_eq!(
            error_of(
                model
                    .ask("x", &Questions::new().choice("q", "pick", ["certain", "uncertain"]))
                    .await
            ),
            "question id=<q> | choice 'uncertain' is reserved for the Uncertain answer"
        );
        assert_eq!(
            error_of(
                model
                    .ask(
                        "x",
                        &Questions::new()
                            .with("q", Question::boolean("ok?").with_uncertainty(Uncertainty::MinConfidence(1.5)))
                    )
                    .await
            ),
            "question id=<q> | uncertainty.minConfidence must be in [0.0, 1.0]"
        );
        assert_eq!(
            error_of(
                model
                    .ask(
                        "x",
                        &Questions::new()
                            .with("q", Question::boolean("ok?").with_uncertainty(Uncertainty::MinConfidence(f64::NAN)))
                    )
                    .await
            ),
            "question id=<q> | uncertainty.minConfidence must be in [0.0, 1.0]"
        );

        assert!(model.calls.lock().is_empty(), "decide must not be reached");
    }

    #[tokio::test]
    async fn ask_delegates_valid_questions_and_returns_the_result() {
        let model = RecordingDecisionModel::default();
        let questions = Questions::new().boolean("a", "ok?").choice("b", "pick", ["x"]);

        let result = model.ask("the state", &questions).await.expect("decide succeeds");

        assert_eq!(result.answers.keys().collect::<Vec<_>>(), ["a", "b"]);
        assert_eq!(result.metadata.latency_ms, 1);
        let calls = model.calls.lock();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].0, DecisionInput::Text("the state".to_owned()));
        assert_eq!(calls[0].1, questions);
    }

    #[tokio::test]
    async fn ask_is_callable_through_a_trait_object() {
        let model: std::sync::Arc<dyn DecisionModel> = std::sync::Arc::new(RecordingDecisionModel::default());
        let result = model
            .ask("x", &Questions::new().boolean("q", "ok?"))
            .await
            .expect("decide succeeds");
        assert_eq!(result.answer("q"), Some(&Answer::Bool(true)));
    }

    #[tokio::test]
    async fn an_uncertainty_at_the_range_bounds_is_valid() {
        let model = RecordingDecisionModel::default();
        for min in [0.0, 1.0] {
            let questions = Questions::new()
                .with("q", Question::boolean("ok?").with_uncertainty(Uncertainty::MinConfidence(min)));
            assert!(model.ask("x", &questions).await.is_ok(), "min={min}");
        }
    }
}
