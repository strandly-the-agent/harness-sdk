//! Public types for the decision-model primitive (`strands-ts/src/decisions/types.ts`).

use indexmap::IndexMap;
use serde::{Serialize, Serializer};

use crate::types::{ContentBlock, Message, Role, Usage};

/// Input state passed to a [`DecisionModel`](super::DecisionModel) invocation.
///
/// The subset of [`InvokeInput`](crate::InvokeInput) that fits a stateless decision: a plain
/// string, a single-turn content list (multimodal), or a full message history.
#[derive(Clone, Debug, PartialEq)]
pub enum DecisionInput {
    /// A plain string, wrapped as one user message.
    Text(String),
    /// Content blocks, wrapped as one user message.
    Blocks(Vec<ContentBlock>),
    /// A message history, passed through as-is.
    Messages(Vec<Message>),
}

impl DecisionInput {
    /// Messages to send to the model (TS `buildMessages`).
    pub fn into_messages(self) -> Vec<Message> {
        match self {
            DecisionInput::Text(text) => vec![Message::user(text)],
            DecisionInput::Blocks(blocks) if blocks.is_empty() => vec![],
            DecisionInput::Blocks(blocks) => vec![Message::new(Role::User, blocks)],
            DecisionInput::Messages(messages) => messages,
        }
    }
}

impl From<&str> for DecisionInput {
    fn from(text: &str) -> Self {
        DecisionInput::Text(text.to_owned())
    }
}
impl From<String> for DecisionInput {
    fn from(text: String) -> Self {
        DecisionInput::Text(text)
    }
}
impl From<Vec<ContentBlock>> for DecisionInput {
    fn from(blocks: Vec<ContentBlock>) -> Self {
        DecisionInput::Blocks(blocks)
    }
}
impl From<ContentBlock> for DecisionInput {
    fn from(block: ContentBlock) -> Self {
        DecisionInput::Blocks(vec![block])
    }
}
impl From<Vec<Message>> for DecisionInput {
    fn from(messages: Vec<Message>) -> Self {
        DecisionInput::Messages(messages)
    }
}
impl From<Message> for DecisionInput {
    fn from(message: Message) -> Self {
        DecisionInput::Messages(vec![message])
    }
}

/// Uncertainty configuration for a [`Question`].
///
/// [`MinConfidence`](Self::MinConfidence) is read only by implementations that return per-answer
/// confidence (e.g. System One models); [`LlmDecisionModel`](super::LlmDecisionModel) ignores it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Uncertainty {
    /// [`Uncertain`] is a permitted answer (the default; TS `uncertainty: true`).
    Allowed,
    /// [`Uncertain`] is disabled; the model must commit to one of the choices (TS `false`).
    Disabled,
    /// Permitted, with a custom minimum confidence in `[0.0, 1.0]` (TS `{ minConfidence }`).
    MinConfidence(f64),
}

/// A single question posed to a [`DecisionModel`](super::DecisionModel).
///
/// The variant constrains the shape of the answer: [`Boolean`](Self::Boolean) yields
/// [`Answer::Bool`], [`Choice`](Self::Choice) yields [`Answer::Choice`]. In both cases
/// [`Answer::Uncertain`] is also possible unless opted out with [`Uncertainty::Disabled`].
#[derive(Clone, Debug, PartialEq)]
pub enum Question {
    /// A yes/no question.
    Boolean {
        /// The question posed to the model, in natural language.
        question: String,
        /// Uncertainty configuration; `None` means [`Uncertainty::Allowed`].
        uncertainty: Option<Uncertainty>,
    },
    /// A question answered with exactly one of `choices`.
    Choice {
        /// The question posed to the model, in natural language.
        question: String,
        /// The allowed answers: non-empty, unique, and none of them `"uncertain"`.
        choices: Vec<String>,
        /// Uncertainty configuration; `None` means [`Uncertainty::Allowed`].
        uncertainty: Option<Uncertainty>,
    },
}

impl Question {
    /// A yes/no question with uncertainty allowed.
    pub fn boolean(question: impl Into<String>) -> Self {
        Question::Boolean {
            question: question.into(),
            uncertainty: None,
        }
    }

    /// A multiple-choice question with uncertainty allowed.
    pub fn choice(question: impl Into<String>, choices: impl IntoIterator<Item = impl Into<String>>) -> Self {
        Question::Choice {
            question: question.into(),
            choices: choices.into_iter().map(Into::into).collect(),
            uncertainty: None,
        }
    }

    /// Replace this question's uncertainty configuration.
    #[must_use]
    pub fn with_uncertainty(mut self, value: Uncertainty) -> Self {
        match &mut self {
            Question::Boolean { uncertainty, .. } | Question::Choice { uncertainty, .. } => {
                *uncertainty = Some(value)
            }
        }
        self
    }

    /// The question text.
    pub fn text(&self) -> &str {
        match self {
            Question::Boolean { question, .. } | Question::Choice { question, .. } => question,
        }
    }

    /// This question's uncertainty configuration, if set.
    pub fn uncertainty(&self) -> Option<Uncertainty> {
        match self {
            Question::Boolean { uncertainty, .. } | Question::Choice { uncertainty, .. } => *uncertainty,
        }
    }

    /// Whether [`Uncertain`] is a permitted answer (TS `uncertaintyAllowed`).
    pub fn uncertainty_allowed(&self) -> bool {
        !matches!(self.uncertainty(), Some(Uncertainty::Disabled))
    }
}

/// An ordered map of question identifier → [`Question`], as passed to
/// [`ask`](super::DecisionModelExt::ask).
///
/// Insertion order is preserved and drives the order of the answers and of the schema sent to the
/// model.
///
/// ```
/// use strands_agents::decisions::{Questions, Uncertainty};
///
/// let questions = Questions::new()
///     .choice("color", "What color is the fruit?", ["red", "green", "blue"])
///     .boolean("is_fruit", "Is the thing talked about a fruit?");
/// assert_eq!(questions.len(), 2);
/// # assert!(questions.get("color").is_some_and(|q| q.uncertainty_allowed()));
/// # let _ = Uncertainty::Disabled;
/// ```
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Questions(IndexMap<String, Question>);

impl Questions {
    /// An empty question map.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add a yes/no question under `id`.
    #[must_use]
    pub fn boolean(self, id: impl Into<String>, question: impl Into<String>) -> Self {
        self.with(id, Question::boolean(question))
    }

    /// Add a multiple-choice question under `id`.
    #[must_use]
    pub fn choice(
        self,
        id: impl Into<String>,
        question: impl Into<String>,
        choices: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        self.with(id, Question::choice(question, choices))
    }

    /// Add a pre-built question under `id`, replacing any question already there.
    #[must_use]
    pub fn with(mut self, id: impl Into<String>, question: Question) -> Self {
        self.0.insert(id.into(), question);
        self
    }

    /// The question registered under `id`.
    pub fn get(&self, id: &str) -> Option<&Question> {
        self.0.get(id)
    }

    /// Number of questions.
    pub fn len(&self) -> usize {
        self.0.len()
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The question ids, in insertion order.
    pub fn ids(&self) -> impl Iterator<Item = &str> {
        self.0.keys().map(String::as_str)
    }

    /// The `(id, question)` pairs, in insertion order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Question)> {
        self.0.iter().map(|(id, q)| (id.as_str(), q))
    }
}

impl<K: Into<String>> FromIterator<(K, Question)> for Questions {
    fn from_iter<I: IntoIterator<Item = (K, Question)>>(iter: I) -> Self {
        Self(iter.into_iter().map(|(id, q)| (id.into(), q)).collect())
    }
}

/// Returned in place of an answer when the decision model declined to commit, with an optional
/// free-form reason supplied by the implementation.
///
/// Serializes as TS `Uncertain.toJSON()` does: `{"uncertain": true, "reason": "…"}`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Uncertain {
    /// Why the model declined to commit, when it said.
    pub reason: Option<String>,
}

impl Uncertain {
    /// Uncertain with no reason.
    pub fn new() -> Self {
        Self::default()
    }

    /// Uncertain with a reason.
    pub fn with_reason(reason: impl Into<String>) -> Self {
        Self {
            reason: Some(reason.into()),
        }
    }
}

impl std::fmt::Display for Uncertain {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.reason {
            Some(reason) => write!(f, "Uncertain({reason})"),
            None => write!(f, "Uncertain"),
        }
    }
}

impl Serialize for Uncertain {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("uncertain", &true)?;
        if let Some(reason) = &self.reason {
            map.serialize_entry("reason", reason)?;
        }
        map.end()
    }
}

/// One question's answer.
///
/// Replaces TS's type-level `AnswerOf<Q>` narrowing: the variant that comes back is determined by
/// the [`Question`] variant that was asked.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(untagged)]
pub enum Answer {
    /// Answer to a [`Question::Boolean`].
    Bool(bool),
    /// Answer to a [`Question::Choice`]: one of its `choices`.
    Choice(String),
    /// The model declined to commit.
    Uncertain(Uncertain),
}

impl Answer {
    /// The boolean answer, or `None` for a choice or uncertain answer.
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Answer::Bool(value) => Some(*value),
            _ => None,
        }
    }

    /// The chosen option, or `None` for a boolean or uncertain answer.
    pub fn as_choice(&self) -> Option<&str> {
        match self {
            Answer::Choice(value) => Some(value),
            _ => None,
        }
    }

    /// The uncertainty sentinel, or `None` when the model committed to an answer.
    pub fn as_uncertain(&self) -> Option<&Uncertain> {
        match self {
            Answer::Uncertain(value) => Some(value),
            _ => None,
        }
    }
}

/// Per-call metadata recorded by a [`DecisionModel`](super::DecisionModel).
#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DecisionMetadata {
    /// Time spent in the underlying model call, in milliseconds.
    pub latency_ms: u64,
}

impl DecisionMetadata {
    /// Metadata recording `latency_ms`.
    pub fn new(latency_ms: u64) -> Self {
        Self { latency_ms }
    }
}

/// Result of [`ask`](super::DecisionModelExt::ask), keyed by the same identifiers as the
/// [`Questions`] that were asked and in the same order.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
#[non_exhaustive]
pub struct DecisionResult {
    /// Per-question answer.
    pub answers: IndexMap<String, Answer>,
    /// Aggregate token usage for the underlying model call(s), when the provider reports it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub usage: Option<Usage>,
    /// Metadata recorded by the implementation.
    pub metadata: DecisionMetadata,
}

impl DecisionResult {
    /// Result carrying `answers` and the call's latency, with no usage.
    pub fn new(answers: IndexMap<String, Answer>, latency_ms: u64) -> Self {
        Self {
            answers,
            usage: None,
            metadata: DecisionMetadata::new(latency_ms),
        }
    }

    /// Attach the underlying call's token usage.
    #[must_use]
    pub fn with_usage(mut self, usage: Usage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// The answer to the question registered under `id`.
    pub fn answer(&self, id: &str) -> Option<&Answer> {
        self.answers.get(id)
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn decision_input_from_impls_normalize_to_messages() {
        assert_eq!(DecisionInput::from("hi").into_messages(), vec![Message::user("hi")]);
        assert_eq!(
            DecisionInput::from(vec![ContentBlock::text("hi")]).into_messages(),
            vec![Message::new(Role::User, vec![ContentBlock::text("hi")])]
        );
        assert_eq!(
            DecisionInput::from(ContentBlock::text("hi")).into_messages(),
            vec![Message::new(Role::User, vec![ContentBlock::text("hi")])]
        );
        let history = vec![Message::user("a"), Message::assistant("b")];
        assert_eq!(
            DecisionInput::from(history.clone()).into_messages(),
            history
        );
        assert_eq!(
            DecisionInput::from(Message::user("a")).into_messages(),
            vec![Message::user("a")]
        );
        // TS treats an empty array as an empty message history, not an empty user turn.
        assert_eq!(DecisionInput::Blocks(vec![]).into_messages(), vec![]);
    }

    #[test]
    fn questions_preserve_insertion_order() {
        let questions = Questions::new()
            .choice("b", "?", ["x"])
            .boolean("a", "?")
            .with("c", Question::boolean("?").with_uncertainty(Uncertainty::Disabled));

        assert_eq!(questions.ids().collect::<Vec<_>>(), ["b", "a", "c"]);
        assert_eq!(questions.len(), 3);
        assert!(!questions.is_empty());
        assert!(questions.get("c").is_some_and(|q| !q.uncertainty_allowed()));
        assert!(questions.get("missing").is_none());
    }

    #[test]
    fn question_accessors_read_both_variants() {
        let boolean = Question::boolean("ok?");
        assert_eq!(boolean.text(), "ok?");
        assert_eq!(boolean.uncertainty(), None);
        assert!(boolean.uncertainty_allowed());

        let choice = Question::choice("c?", ["red"]).with_uncertainty(Uncertainty::MinConfidence(0.5));
        assert_eq!(choice.text(), "c?");
        assert_eq!(choice.uncertainty(), Some(Uncertainty::MinConfidence(0.5)));
        assert!(choice.uncertainty_allowed());
    }

    #[test]
    fn uncertain_display_matches_ts_to_string() {
        assert_eq!(Uncertain::new().to_string(), "Uncertain");
        assert_eq!(Uncertain::with_reason("no color").to_string(), "Uncertain(no color)");
    }

    #[test]
    fn uncertain_serializes_like_ts_to_json() {
        assert_eq!(serde_json::to_value(Uncertain::new()).unwrap(), json!({ "uncertain": true }));
        assert_eq!(
            serde_json::to_value(Uncertain::with_reason("why")).unwrap(),
            json!({ "uncertain": true, "reason": "why" })
        );
    }

    #[test]
    fn answer_serializes_untagged_like_ts() {
        assert_eq!(serde_json::to_value(Answer::Bool(true)).unwrap(), json!(true));
        assert_eq!(serde_json::to_value(Answer::Choice("red".into())).unwrap(), json!("red"));
        assert_eq!(
            serde_json::to_value(Answer::Uncertain(Uncertain::with_reason("r"))).unwrap(),
            json!({ "uncertain": true, "reason": "r" })
        );
    }

    #[test]
    fn answer_accessors_narrow_by_variant() {
        assert_eq!(Answer::Bool(false).as_bool(), Some(false));
        assert_eq!(Answer::Bool(false).as_choice(), None);
        assert_eq!(Answer::Choice("red".into()).as_choice(), Some("red"));
        assert_eq!(Answer::Choice("red".into()).as_bool(), None);
        assert_eq!(
            Answer::Uncertain(Uncertain::new()).as_uncertain(),
            Some(&Uncertain::new())
        );
        assert_eq!(Answer::Bool(true).as_uncertain(), None);
    }

    #[test]
    fn decision_result_serializes_like_ts() {
        let answers: IndexMap<String, Answer> = [
            ("color".to_owned(), Answer::Choice("red".to_owned())),
            ("is_fruit".to_owned(), Answer::Bool(true)),
        ]
        .into_iter()
        .collect();

        let bare = DecisionResult::new(answers.clone(), 42);
        assert_eq!(
            serde_json::to_value(&bare).unwrap(),
            json!({
                "answers": { "color": "red", "is_fruit": true },
                "metadata": { "latencyMs": 42 }
            })
        );
        assert_eq!(bare.answer("color"), Some(&Answer::Choice("red".to_owned())));

        let with_usage = DecisionResult::new(answers, 42).with_usage(Usage {
            input_tokens: 3,
            output_tokens: 2,
            total_tokens: 5,
            ..Usage::default()
        });
        assert_eq!(
            serde_json::to_value(&with_usage).unwrap(),
            json!({
                "answers": { "color": "red", "is_fruit": true },
                "usage": { "inputTokens": 3, "outputTokens": 2, "totalTokens": 5 },
                "metadata": { "latencyMs": 42 }
            })
        );
    }

    #[test]
    fn questions_from_iter_collects_in_order() {
        let questions: Questions = [("a", Question::boolean("?")), ("b", Question::choice("?", ["x"]))]
            .into_iter()
            .collect();
        assert_eq!(questions.iter().map(|(id, _)| id).collect::<Vec<_>>(), ["a", "b"]);
    }
}
