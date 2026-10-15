//! Message/context injection (`strands-ts/src/injection/`): folding just-in-time text into the
//! model input for one call, ephemerally — the model sees the augmented conversation while the
//! agent's durable history is never touched.
//!
//! TS delivers injection as an `InvokeModelStage.Input` middleware handler; Rust uses the
//! equivalent seam, [`BeforeModelCallEvent`](crate::hooks::BeforeModelCallEvent) — its `messages`
//! are the per-call copy of the conversation and its `dynamic_trailing_blocks` carries TS's
//! `dynamicTrailingBlocks` accumulator to the provider's cache-point placement.
//!
//! This module is the single home for the delivery primitives: [`is_user_turn`],
//! [`fold_into_last_user_message`], the [`Trigger`] gate (TS `resolveTrigger`) and the
//! [`MessageInjection`] engine (TS `createInjectionMiddleware`), plus the
//! [`xml`] escaping helpers. Reach injection through
//! [`ContextInjector`](crate::plugins::ContextInjector) or
//! [`MemoryManager`](crate::memory::MemoryManager) rather than wiring it by hand.

pub mod message_injection;
pub mod types;
pub mod xml;

pub use message_injection::{fold_into_last_user_message, is_user_turn, FoldOutcome, MessageInjection};
pub use types::{InjectionContext, InjectionTrigger, Trigger};
pub use xml::{escape_xml_attr, escape_xml_text};

#[cfg(test)]
pub(crate) mod tests {
    use crate::types::{ContentBlock, Message, Role, ToolResultBlock, ToolResultContent};

    /// A user turn carrying a tool result instead of a fresh ask.
    pub(crate) fn user_with_tool_result() -> Message {
        Message::new(
            Role::User,
            vec![ContentBlock::ToolResult(ToolResultBlock::success(
                "t1",
                vec![ToolResultContent::Text("done".to_owned())],
            ))],
        )
    }
}
