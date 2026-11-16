//! Human approval of tool calls (TS `vended-interventions/hitl/`).

mod classifier;

use std::collections::HashSet;
use std::fmt;
use std::io::{self, BufRead, Write};
use std::sync::{Arc, Mutex, PoisonError};

use async_trait::async_trait;
use futures::future::BoxFuture;

use classifier::create_llm_risk_classifier;
pub use classifier::{
    ClassifierResult, HumanInTheLoopClassifier, LlmClassifierConfig, LlmClassifierConfigBuilder,
    llm_classifier_config_builder,
};
use tokio::sync::oneshot;

use crate::error::{Error, Result};
use crate::hooks::BeforeToolCallEvent;
use crate::interventions::{
    BeforeToolCallAction, Evaluate, InterventionHandler, confirm, default_evaluate, deny, proceed,
};
use crate::models::model::js_trim;
use crate::state_store::StateStore;
use crate::types::json::{JsonValue, to_js_string};

const TRUSTED_TOOLS_KEY: &str = "hitl:trustedTools";

/// Asks the human and returns their answer; `None` (no answer) denies the call.
pub type AskFn = Arc<dyn Fn(String) -> BoxFuture<'static, Result<Option<JsonValue>>> + Send + Sync>;

/// How [`HumanInTheLoop`] collects the human's answer inline (TS `ask`).
#[derive(Clone)]
#[non_exhaustive]
pub enum Ask {
    /// Prompt on the terminal and read a line from standard input (TS `'stdio'`).
    Stdio,
    /// Call the function with the prompt.
    Callback(AskFn),
}

impl Ask {
    /// An [`Ask::Callback`] from an async function.
    pub fn callback<F, Fut>(ask: F) -> Self
    where
        F: Fn(String) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Option<JsonValue>>> + Send + 'static,
    {
        Self::Callback(Arc::new(move |prompt| Box::pin(ask(prompt))))
    }
}

impl fmt::Debug for Ask {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Stdio => "Stdio",
            Self::Callback(_) => "Callback",
        })
    }
}

/// Decides which tool calls outside `allowed_tools` need approval (TS `classifier`).
#[derive(Clone)]
#[non_exhaustive]
pub enum Classifier {
    /// `true` is the built-in LLM risk classifier with its defaults (model order:
    /// `agent.aux_model > agent.model`); `false` is no classifier.
    Enabled(bool),
    /// The built-in LLM risk classifier with this configuration.
    Llm(LlmClassifierConfig),
    /// A custom classifier.
    Function(HumanInTheLoopClassifier),
}

impl Classifier {
    /// A [`Classifier::Function`] from an async function. The function reads what it needs
    /// from the event before it returns the future, which can't borrow the event.
    ///
    /// ```
    /// use strands::vended_interventions::hitl::{Classifier, ClassifierResult, HumanInTheLoopConfig};
    ///
    /// let classifier = Classifier::function(|event| {
    ///     let risky = event.tool_use().name() == "shell";
    ///     async move { Ok(ClassifierResult::new(risky)) }
    /// });
    /// let config = HumanInTheLoopConfig::builder().classifier(classifier).build();
    /// ```
    pub fn function<F, Fut>(classify: F) -> Self
    where
        F: Fn(&BeforeToolCallEvent) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ClassifierResult>> + Send + 'static,
    {
        Self::Function(Arc::new(move |event| Box::pin(classify(event))))
    }
}

impl From<bool> for Classifier {
    fn from(enabled: bool) -> Self {
        Self::Enabled(enabled)
    }
}

impl From<LlmClassifierConfig> for Classifier {
    fn from(config: LlmClassifierConfig) -> Self {
        Self::Llm(config)
    }
}

impl From<HumanInTheLoopClassifier> for Classifier {
    fn from(classifier: HumanInTheLoopClassifier) -> Self {
        Self::Function(classifier)
    }
}

impl fmt::Debug for Classifier {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Enabled(enabled) => f.debug_tuple("Enabled").field(enabled).finish(),
            Self::Llm(config) => f.debug_tuple("Llm").field(config).finish(),
            Self::Function(_) => f.write_str("Function"),
        }
    }
}

/// Configuration of [`HumanInTheLoop`] (TS `HumanInTheLoopConfig`).
#[derive(Clone, Default, bon::Builder)]
#[builder(state_mod(vis = "pub"))]
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
pub struct HumanInTheLoopConfig {
    /// Tools that run without approval. `*` allows every tool; `!name` always asks for `name`,
    /// even with `*` and even when the tool was trusted.
    #[builder(default, with = |tools: impl IntoIterator<Item = impl Into<String>>| tools.into_iter().map(Into::into).collect())]
    pub allowed_tools: Vec<String>,
    /// Decides whether a tool call outside `allowed_tools` needs approval; without one every
    /// such call does.
    #[builder(into)]
    pub classifier: Option<Classifier>,
    /// Lets the human trust a tool for the rest of the session by answering `t` or `trust`.
    #[builder(default)]
    pub enable_trust: bool,
    /// Replaces the `t`/`trust` check.
    #[builder(with = |evaluate: impl Fn(&JsonValue) -> bool + Send + Sync + 'static| Arc::new(evaluate) as Evaluate)]
    pub evaluate_trust: Option<Evaluate>,
    /// Replaces the default approval check (`true`, `y` or `yes` approves).
    #[builder(with = |evaluate: impl Fn(&JsonValue) -> bool + Send + Sync + 'static| Arc::new(evaluate) as Evaluate)]
    pub evaluate: Option<Evaluate>,
    /// Collects the answer inline. Without it the agent pauses with an interrupt and resumes
    /// with the answer.
    pub ask: Option<Ask>,
}

opaque_builder_debug!(HumanInTheLoopConfigBuilder, human_in_the_loop_config_builder);

impl fmt::Debug for HumanInTheLoopConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HumanInTheLoopConfig")
            .field("allowed_tools", &self.allowed_tools)
            .field("classifier", &self.classifier)
            .field("enable_trust", &self.enable_trust)
            .field("ask", &self.ask)
            .finish_non_exhaustive()
    }
}

/// Requires human approval before tools run (TS `HumanInTheLoop`).
///
/// ```
/// use strands::vended_interventions::hitl::{HumanInTheLoop, HumanInTheLoopConfig};
///
/// let hitl = HumanInTheLoop::new(
///     HumanInTheLoopConfig::builder()
///         .allowed_tools(["read", "!shell"])
///         .enable_trust(true)
///         .build(),
/// );
/// let agent = strands::Agent::builder().intervention(hitl).build()?;
/// # Ok::<(), strands::Error>(())
/// ```
pub struct HumanInTheLoop {
    allowed_tools: HashSet<String>,
    classifier: Option<HumanInTheLoopClassifier>,
    enable_trust: bool,
    evaluate_trust: Evaluate,
    evaluate: Evaluate,
    ask: Option<AskFn>,
    classified_tool_use_ids: Mutex<HashSet<String>>,
}

impl Default for HumanInTheLoop {
    /// The handler with the default config (TS `new HumanInTheLoop()`).
    fn default() -> Self {
        Self::new(HumanInTheLoopConfig::default())
    }
}

impl HumanInTheLoop {
    /// The handler's name.
    pub const NAME: &'static str = "strands:human-in-the-loop";

    /// A handler from `config`.
    pub fn new(config: HumanInTheLoopConfig) -> Self {
        let allowed_tools: HashSet<String> = config.allowed_tools.into_iter().collect();
        let classifier = match config.classifier {
            None | Some(Classifier::Enabled(false)) => None,
            Some(Classifier::Enabled(true)) => {
                Some(create_llm_risk_classifier(LlmClassifierConfig::default()))
            }
            Some(Classifier::Llm(config)) => Some(create_llm_risk_classifier(config)),
            Some(Classifier::Function(classifier)) => Some(classifier),
        };
        if classifier.is_some() && allowed_tools.contains("*") {
            tracing::warn!(
                "classifier has no effect when allowedTools contains \"*\" — all tools are already allowed"
            );
        }
        let ask = config.ask.map(|ask| match ask {
            Ask::Stdio => stdio_ask(config.enable_trust),
            Ask::Callback(ask) => ask,
        });
        Self {
            allowed_tools,
            classifier,
            enable_trust: config.enable_trust,
            evaluate_trust: config
                .evaluate_trust
                .unwrap_or_else(|| Arc::new(is_trust_response)),
            evaluate: config.evaluate.unwrap_or_else(|| Arc::new(default_evaluate)),
            ask,
            classified_tool_use_ids: Mutex::new(HashSet::new()),
        }
    }

    async fn requires_approval(&self, event: &BeforeToolCallEvent) -> ClassifierResult {
        let tool_name = event.tool_use().name();
        if self.allowed_tools.contains(&format!("!{tool_name}")) {
            return ClassifierResult::new(true);
        }
        if self.enable_trust
            && trusted_tools(event.agent().app_state())
                .iter()
                .any(|tool| tool == tool_name)
        {
            return ClassifierResult::new(false);
        }
        if self.allowed_tools.contains("*") || self.allowed_tools.contains(tool_name) {
            return ClassifierResult::new(false);
        }
        let Some(classifier) = &self.classifier else {
            return ClassifierResult::new(true);
        };
        // A call that was classified before is being resumed: it needed approval then.
        let first_time = self
            .classified_tool_use_ids
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(event.tool_use().tool_use_id().to_owned());
        if !first_time {
            return ClassifierResult::new(true);
        }
        let classified = classifier(event);
        match classified.await {
            Ok(result) => result,
            Err(error) => {
                tracing::warn!(
                    error = %error,
                    "tool=<{}> | classifier failed, defaulting to approval required",
                    event.tool_use().name()
                );
                ClassifierResult::new(true)
            }
        }
    }
}

impl fmt::Debug for HumanInTheLoop {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("HumanInTheLoop")
            .field("allowed_tools", &self.allowed_tools)
            .field("enable_trust", &self.enable_trust)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl InterventionHandler for HumanInTheLoop {
    fn name(&self) -> &str {
        Self::NAME
    }

    async fn before_tool_call(&self, event: &mut BeforeToolCallEvent) -> Result<BeforeToolCallAction> {
        let tool_name = event.tool_use().name().to_owned();
        let classified = self.requires_approval(event).await;
        if !classified.requires_human_in_the_loop {
            return Ok(proceed().into());
        }
        let reason = match classified.reason.as_deref() {
            Some(reason) if !reason.is_empty() => format!(" — {reason}"),
            _ => String::new(),
        };
        let input = to_js_string(event.tool_use().input()).map_err(Error::other)?;
        let prompt = format!("Approve \"{tool_name}\"{reason}?\n  Input: {input}");
        let can_trust = self.enable_trust && !self.allowed_tools.contains(&format!("!{tool_name}"));
        let app_state = event.agent().app_state().clone();

        let Some(ask) = &self.ask else {
            let evaluate_trust = Arc::clone(&self.evaluate_trust);
            let evaluate = Arc::clone(&self.evaluate);
            return Ok(confirm(prompt)
                .with_evaluate(move |response| {
                    if can_trust && evaluate_trust(response) {
                        trust_tool(&app_state, &tool_name);
                        return true;
                    }
                    evaluate(response)
                })
                .into());
        };
        // An inline caller can't resume an interrupt, so a missing answer fails closed.
        let Some(response) = ask(prompt).await? else {
            return Ok(deny(format!(
                "Tool \"{tool_name}\" denied: approval callback returned no response."
            ))
            .into());
        };
        if can_trust && (self.evaluate_trust)(&response) {
            trust_tool(&app_state, &tool_name);
            return Ok(proceed().into());
        }
        if (self.evaluate)(&response) {
            Ok(proceed().into())
        } else {
            Ok(deny(format!(
                "Tool \"{tool_name}\" denied: the human rejected the call."
            ))
            .into())
        }
    }
}

fn is_trust_response(response: &JsonValue) -> bool {
    response
        .as_str()
        .is_some_and(|text| matches!(js_trim(&text.to_lowercase()), "t" | "trust"))
}

/// The tools trusted for this session; a malformed stored value reads as none.
fn trusted_tools(app_state: &StateStore) -> Vec<String> {
    app_state
        .get::<Vec<String>>(TRUSTED_TOOLS_KEY)
        .ok()
        .flatten()
        .unwrap_or_default()
}

fn trust_tool(app_state: &StateStore, tool_name: &str) {
    let mut trusted = trusted_tools(app_state);
    if trusted.iter().any(|tool| tool == tool_name) {
        return;
    }
    trusted.push(tool_name.to_owned());
    if let Err(error) = app_state.set(TRUSTED_TOOLS_KEY, trusted) {
        tracing::warn!(error = %error, "tool=<{tool_name}> | failed to store the trusted tool");
    }
}

type StdinRead = oneshot::Receiver<io::Result<String>>;

/// The stdin read in flight. Stdin is per process, so every [`Ask::Stdio`] handler queues
/// here, and a read whose prompt was dropped stays for the next prompt to adopt.
static STDIN_READ: tokio::sync::Mutex<Option<StdinRead>> = tokio::sync::Mutex::const_new(None);

/// Prompts on the terminal, one question at a time (TS `createStdioAsk`).
fn stdio_ask(include_trust: bool) -> AskFn {
    let options = if include_trust { "(y/n/t)" } else { "(y/n)" };
    Arc::new(move |prompt| {
        Box::pin(async move {
            let show = || {
                let mut stdout = io::stdout().lock();
                write!(stdout, "{prompt} {options}: ")?;
                stdout.flush()
            };
            let answer = ask_in_turn(&STDIN_READ, show, read_stdin_line).await?;
            Ok(Some(JsonValue::String(js_trim(&answer).to_owned())))
        })
    })
}

/// Waits for its turn, shows the prompt and returns the next line `read` delivers. Nothing
/// but the queue is held while waiting for the line. A dropped prompt's read is adopted only
/// while it still waits: a line it already delivered answered the dropped question.
async fn ask_in_turn(
    queue: &tokio::sync::Mutex<Option<StdinRead>>,
    show: impl FnOnce() -> io::Result<()>,
    read: impl FnOnce() -> StdinRead,
) -> Result<String> {
    let mut pending = queue.lock().await;
    if pending
        .as_mut()
        .is_some_and(|read| !matches!(read.try_recv(), Err(oneshot::error::TryRecvError::Empty)))
    {
        *pending = None;
    }
    show().map_err(Error::other)?;
    let answer = pending.get_or_insert_with(read).await;
    *pending = None;
    answer.map_err(Error::other)?.map_err(Error::other)
}

/// Reads one line of stdin on its own thread, so neither a dropped prompt nor the runtime's
/// shutdown waits for the human.
fn read_stdin_line() -> StdinRead {
    let (answer, read) = oneshot::channel();
    let reader = move || {
        let mut line = String::new();
        let result = io::stdin().lock().read_line(&mut line).map(|_| line);
        // The queue keeps the receiver, so the line reaches the prompt that waits or the next one.
        drop(answer.send(result));
    };
    // If the thread can't start, `answer` is dropped and the prompt fails with a receive error.
    drop(
        std::thread::Builder::new()
            .name("strands-hitl-stdin".to_owned())
            .spawn(reader),
    );
    read
}

#[cfg(test)]
mod tests;
