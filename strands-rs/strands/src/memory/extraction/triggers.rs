//! The built-in extraction triggers (TS `memory/extraction/triggers.ts`).

use std::sync::atomic::{AtomicU64, Ordering};

use super::types::{ExtractionTrigger, ExtractionTriggerContext};
use crate::error::{Error, Result};
use crate::hooks::{AfterInvocationEvent, HookCallbackOptions, HookOrder};

/// Fires after every invocation (TS `InvocationTrigger`).
#[derive(Debug, Clone, Copy, Default)]
#[non_exhaustive]
pub struct InvocationTrigger;

impl InvocationTrigger {
    /// The trigger name (TS `InvocationTrigger.name`).
    pub const NAME: &'static str = "invocation";

    /// The trigger.
    pub fn new() -> Self {
        Self
    }
}

impl ExtractionTrigger for InvocationTrigger {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn attach(&self, context: ExtractionTriggerContext<'_>) {
        let fire = context.fire;
        // After the SDK's own after-invocation hooks, so extraction sees the settled turn.
        context.agent.hooks().add_hook_with(
            HookCallbackOptions::builder().order(HookOrder::SDK_LAST).build(),
            move |event: &mut AfterInvocationEvent<'_>| fire.fire(event.invocation_context().cloned()),
        );
    }
}

/// Options for [`IntervalTrigger`] (TS `IntervalTriggerOptions`).
#[expect(
    clippy::exhaustive_structs,
    reason = "config struct: struct-literal construction"
)]
#[derive(Debug, Clone, PartialEq, Eq, bon::Builder)]
#[builder(state_mod(vis = "pub"), derive(Debug))]
pub struct IntervalTriggerOptions {
    /// Fire every this many invocations; at least 1.
    pub turns: u64,
}

/// Fires every `turns` invocations (TS `IntervalTrigger`).
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct IntervalTrigger {
    turns: u64,
}

impl IntervalTrigger {
    /// The trigger name (TS `IntervalTrigger.name`).
    pub const NAME: &'static str = "interval";

    /// A trigger firing every `options.turns` invocations.
    ///
    /// # Errors
    ///
    /// `turns` is 0.
    #[expect(
        clippy::needless_pass_by_value,
        reason = "takes its options by value like every other options parameter"
    )]
    pub fn new(options: IntervalTriggerOptions) -> Result<Self> {
        if options.turns < 1 {
            return Err(Error::other(format!(
                "IntervalTrigger: turns must be a positive integer, got {}",
                options.turns
            )));
        }
        Ok(Self { turns: options.turns })
    }

    /// The default trigger; `turns` is a non-zero constant.
    pub(crate) const fn every(turns: u64) -> Self {
        Self { turns }
    }
}

impl ExtractionTrigger for IntervalTrigger {
    fn name(&self) -> &str {
        Self::NAME
    }

    fn attach(&self, context: ExtractionTriggerContext<'_>) {
        let fire = context.fire;
        let turns = self.turns;
        // One counter per attach, so stores sharing a trigger still fire independently.
        let count = AtomicU64::new(0);
        context.agent.hooks().add_hook_with(
            HookCallbackOptions::builder().order(HookOrder::SDK_LAST).build(),
            move |event: &mut AfterInvocationEvent<'_>| {
                let count = count.fetch_add(1, Ordering::Relaxed).wrapping_add(1);
                if count.is_multiple_of(turns) {
                    fire.fire(event.invocation_context().cloned());
                }
            },
        );
    }
}
