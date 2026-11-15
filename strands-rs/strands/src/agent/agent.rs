//! The agent (TS `agent/agent.ts`).

use std::collections::HashSet;
use std::fmt;
use std::ops::{Deref, DerefMut};
use std::sync::Arc;

use crate::agent::agent_delegation;
use crate::agent::config::{AgentBuilder, AgentConfig};
use crate::agent::invoke::{InvokeArgs, InvokeOptions};
use crate::agent::local_agent::{LocalAgent, LocalAgentParts};
use crate::agent::printer::AgentPrinter;
use crate::agent::stream::AgentStream;
use crate::cancel::{CancelHandle, CancelScope};
use crate::error::{Result, UnsupportedError};
use crate::hooks::events::{AfterInvocationEvent, AfterModelCallEvent, InitializedEvent};
use crate::hooks::registry::HookRegistry;
use crate::models::model::{Model, ModelSource};
use crate::retry::{DefaultModelRetryStrategy, RetryStrategy, Sleeper, retry_model};
use crate::state_store::StateStore;
use crate::tools::ToolSource;
use crate::tools::executors::ToolExecutor;
use crate::tools::registry::ToolRegistry;
use crate::tools::structured_output::StructuredOutputSchema;
use crate::types::agent::AgentResult;

/// The agent's name when none is configured (TS `DEFAULT_AGENT_NAME`).
pub const DEFAULT_AGENT_NAME: &str = "Strands Agent";

/// The agent's id when none is configured (TS `DEFAULT_AGENT_ID`).
pub const DEFAULT_AGENT_ID: &str = "agent";

/// An agent: a model, a system prompt and tools, run in a loop where the model decides which
/// tools to call (TS `Agent`).
///
/// Build one with [`Agent::builder`] or [`Agent::new`], then call [`invoke`](Self::invoke) for a
/// result or [`stream`](Self::stream) for the events as they happen. Methods take `&mut self`;
/// share an agent across tasks behind `Arc<tokio::sync::Mutex<Agent>>`.
///
/// `Agent` dereferences to [`LocalAgent`], the state hooks see (TS `Agent implements
/// LocalAgent`): messages, system prompt, models, tools, state and hook registration.
///
/// ```
/// use strands::{Agent, ContentBlock};
/// use strands::testing::MockModel;
///
/// # #[tokio::main(flavor = "current_thread")]
/// # async fn main() -> Result<(), strands::Error> {
/// let model = MockModel::new();
/// model.add_turn(vec![ContentBlock::from("Hello!")]);
/// let mut agent = Agent::builder()
///     .model(model)
///     .system_prompt("You are a concise assistant.")
///     .printer(false)
///     .build()?;
/// let result = agent.invoke("Hi").await?;
/// assert_eq!(result.to_string(), "Hello!");
/// assert_eq!(agent.messages().len(), 2);
/// agent.shutdown().await
/// # }
/// ```
pub struct Agent {
    pub(crate) local: LocalAgent,
    pub(crate) tool_executor: ToolExecutor,
    pub(crate) structured_output_schema: Option<StructuredOutputSchema>,
    retry_strategies: Vec<RetryStrategy>,
    initialized: bool,
    pub(crate) cancel: CancelScope,
}

impl Agent {
    /// A builder for an agent; see [`AgentConfig`] for the keys.
    pub fn builder() -> AgentBuilder {
        AgentConfig::builder()
    }

    /// Creates an agent from a configuration (TS `new Agent(config)`). Nothing runs until the
    /// first [`invoke`](Self::invoke), [`stream`](Self::stream) or
    /// [`initialize`](Self::initialize).
    ///
    /// # Errors
    ///
    /// [`ToolValidationError`](crate::error::ToolValidationError) when a tool's name is invalid,
    /// duplicated or its description is empty; [`UnsupportedError`] for a key this version
    /// doesn't support yet (`baggage_attributes`) and for a model given as a Bedrock model id.
    pub fn new(config: AgentConfig) -> Result<Self> {
        if config
            .baggage_attributes
            .as_ref()
            .is_some_and(|attributes| !attributes.is_empty())
        {
            return Err(
                UnsupportedError::new("AgentConfig.baggage_attributes is not supported in Rust yet").into(),
            );
        }
        let model = config.model.map(resolve_model).transpose()?;
        let aux_model = config.aux_model.map(resolve_model).transpose()?;
        let tools = config.tools.into_iter().flat_map(ToolSource::into_tools);
        let tool_registry = ToolRegistry::with_tools(tools)?;
        let retry_strategies = config
            .retry_strategy
            .unwrap_or_else(|| vec![DefaultModelRetryStrategy::default().into()]);
        warn_on_duplicate_retry_strategies(&retry_strategies);
        let hooks = Arc::new(HookRegistry::new());
        if config.printer {
            AgentPrinter::stdout().install(&hooks);
        }
        let local = LocalAgent::new(LocalAgentParts {
            id: config.id.unwrap_or_else(|| DEFAULT_AGENT_ID.to_owned()),
            name: config.name.unwrap_or_else(|| DEFAULT_AGENT_NAME.to_owned()),
            description: config.description,
            messages: config.messages,
            system_prompt: config.system_prompt,
            model,
            aux_model,
            tool_registry,
            app_state: config.app_state.map(StateStore::from).unwrap_or_default(),
            model_state: config.model_state.map(StateStore::from).unwrap_or_default(),
            hooks,
        });
        Ok(Self {
            local,
            tool_executor: config.tool_executor.unwrap_or_default(),
            structured_output_schema: config.structured_output_schema,
            retry_strategies,
            initialized: false,
            cancel: CancelScope::default(),
        })
    }

    /// Prepares the agent: registers the retry, delegation and model hooks and fires
    /// `InitializedEvent` (TS `initialize`, with the built-in plugins in the TS order). Runs once;
    /// the first `invoke` or `stream` calls it.
    ///
    /// # Errors
    ///
    /// A plain error when a stateful model is combined with a delegating agent tool (TS text).
    pub async fn initialize(&mut self) -> Result<()> {
        if self.initialized {
            return Ok(());
        }
        self.register_retry_strategies();
        agent_delegation::register(&self.local)?;
        self.register_model_hook();
        let hooks = Arc::clone(self.local.hooks());
        let mut event = InitializedEvent::new(&mut self.local);
        hooks.invoke::<InitializedEvent<'static>>(&mut event).await;
        self.initialized = true;
        Ok(())
    }

    /// Runs the agent on `args` and returns the result (TS `invoke`). Drains
    /// [`stream`](Self::stream).
    ///
    /// # Errors
    ///
    /// A model error the retry strategies didn't recover from, or invalid input (a blank prompt,
    /// interrupt responses mixed with other content).
    pub async fn invoke(&mut self, args: impl Into<InvokeArgs>) -> Result<AgentResult> {
        self.invoke_with(args, InvokeOptions::default()).await
    }

    /// [`invoke`](Self::invoke) with options.
    ///
    /// # Errors
    ///
    /// As [`invoke`](Self::invoke).
    pub async fn invoke_with(
        &mut self,
        args: impl Into<InvokeArgs>,
        options: InvokeOptions,
    ) -> Result<AgentResult> {
        self.stream_with(args, options).into_result().await
    }

    /// Runs the agent on `args` and streams its events (TS `stream`). The loop advances as the
    /// stream is polled; dropping the stream stops it.
    pub fn stream(&mut self, args: impl Into<InvokeArgs>) -> AgentStream<'_> {
        self.stream_with(args, InvokeOptions::default())
    }

    /// [`stream`](Self::stream) with options.
    pub fn stream_with(&mut self, args: impl Into<InvokeArgs>, options: InvokeOptions) -> AgentStream<'_> {
        let args = args.into();
        AgentStream::generate(move |yielder| self.run_invocation(args, options, yielder))
    }

    /// A handle that cancels whichever invocation is running (TS `agent.cancel`). Cancelling
    /// while the agent is idle does nothing.
    #[must_use]
    pub fn cancel_handle(&self) -> CancelHandle {
        self.cancel.handle()
    }

    /// How the agent runs the tools of one turn.
    #[must_use]
    pub fn tool_executor(&self) -> ToolExecutor {
        self.tool_executor
    }

    /// Replaces how the agent runs the tools of one turn.
    pub fn set_tool_executor(&mut self, tool_executor: impl Into<ToolExecutor>) {
        self.tool_executor = tool_executor.into();
    }

    /// Finishes the agent's pending work (TS `shutdown`). Nothing is pending in this version.
    ///
    /// # Errors
    ///
    /// None yet; the signature leaves room for subsystems that flush on shutdown.
    #[expect(
        clippy::unused_async,
        reason = "memory and deferred cleanup flush here once they exist"
    )]
    pub async fn shutdown(&mut self) -> Result<()> {
        Ok(())
    }

    fn register_retry_strategies(&self) {
        for strategy in &self.retry_strategies {
            let strategy = Arc::clone(strategy.model_strategy());
            let cancel = self.cancel.handle();
            self.local
                .hooks()
                .add_async_hook::<AfterModelCallEvent<'static>>(move |event| {
                    let strategy = Arc::clone(&strategy);
                    let sleeper = CancellableSleeper(cancel.clone());
                    Box::pin(async move {
                        let retry = retry_model(
                            strategy.as_ref(),
                            &sleeper,
                            event.attempt_count(),
                            event.error(),
                            event.retry(),
                        )
                        .await;
                        event.set_retry(retry);
                    })
                });
        }
    }

    /// TS `ModelPlugin`: a stateful model keeps the conversation server-side, so local history
    /// is cleared after each invocation.
    fn register_model_hook(&self) {
        let model = Arc::clone(self.local.model());
        self.local
            .hooks()
            .add_hook(move |event: &mut AfterInvocationEvent<'_>| {
                if model.stateful() {
                    event.agent_mut().messages_mut().clear();
                    tracing::debug!("cleared messages for server-managed conversation");
                }
            });
    }
}

impl Deref for Agent {
    type Target = LocalAgent;

    fn deref(&self) -> &LocalAgent {
        &self.local
    }
}

impl DerefMut for Agent {
    fn deref_mut(&mut self) -> &mut LocalAgent {
        &mut self.local
    }
}

impl fmt::Debug for Agent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Agent")
            .field("id", &self.local.id())
            .field("name", &self.local.name())
            .finish_non_exhaustive()
    }
}

fn resolve_model(source: ModelSource) -> Result<Arc<dyn Model>> {
    match source {
        ModelSource::Model(model) => Ok(model),
        ModelSource::Id(id) => Err(UnsupportedError::new(format!(
            "model_id=<{id}> | a model id needs BedrockModel, which is not supported in Rust yet"
        ))
        .into()),
    }
}

/// TS `warnOnDuplicateRetryStrategyTypes`, keyed by strategy name (Rust has no constructor name).
fn warn_on_duplicate_retry_strategies(strategies: &[RetryStrategy]) {
    let mut seen = HashSet::new();
    for strategy in strategies {
        if !seen.insert(strategy.name()) {
            tracing::warn!(
                "retry_strategy_type=<{}> | multiple instances provided; only the first will be used",
                strategy.name()
            );
        }
    }
}

/// Waits between model attempts, ending early when the invocation is cancelled (TS `sleep(ms,
/// cancelSignal)`).
struct CancellableSleeper(CancelHandle);

#[async_trait::async_trait]
impl Sleeper for CancellableSleeper {
    async fn sleep(&self, duration: std::time::Duration) {
        let signal = self.0.signal();
        if signal.is_cancelled() {
            return;
        }
        tokio::select! {
            () = tokio::time::sleep(duration) => {}
            () = signal.cancelled() => {}
        }
    }
}
