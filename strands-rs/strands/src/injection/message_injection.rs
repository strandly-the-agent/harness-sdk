//! The injection engine (TS `injection/message-injection.ts`): folds text into the latest user
//! message of one model call, leaving the agent's history untouched.

use super::types::{InjectionContext, InjectionMiddlewareOptions};
use crate::middleware::{InvokeModelContext, InvokeModelStage, Middleware, MiddlewareStage};
use crate::types::messages::{ContentBlock, Message, Role, TextBlock};

/// An `InvokeModelStage` Input handler that folds the rendered text into the latest user message
/// for this call only (TS `createInjectionMiddleware`). A trigger that doesn't fire, blank text
/// and a failing renderer all leave the context unchanged (fail open).
pub(crate) fn create_injection_middleware(opts: InjectionMiddlewareOptions) -> Middleware<InvokeModelStage> {
    InvokeModelStage::input(move |context| {
        let opts = opts.clone();
        async move { Ok(apply_injection(&opts, context).await) }
    })
}

/// The handler [`create_injection_middleware`] installs.
pub(crate) async fn apply_injection(
    opts: &InjectionMiddlewareOptions,
    mut context: InvokeModelContext,
) -> InvokeModelContext {
    let injection_context = InjectionContext::new(context.messages.clone(), context.agent().clone());
    if !opts.trigger.fires(&injection_context) {
        return context;
    }
    let text = match (opts.render_content)(injection_context).await {
        Ok(text) => text,
        Err(error) => {
            tracing::warn!("reason=<{error}> | injection renderContent threw; skipping injection");
            return context;
        }
    };
    let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
        return context;
    };
    let folded = fold_into_last_user_message(std::mem::take(&mut context.messages), &text);
    context.messages = folded.messages;
    // Accumulates: several producers may append on one call, and the cache point precedes all of it.
    context.dynamic_trailing_blocks = Some(context.dynamic_trailing_blocks.unwrap_or(0) + folded.appended);
    context
}

/// Whether the latest message is a fresh user ask: a user message with no tool result (TS
/// `isUserTurn`).
pub(crate) fn is_user_turn(messages: &[Message]) -> bool {
    messages.last().is_some_and(|last| {
        last.role() == Role::User
            && !last
                .content()
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult(_)))
    })
}

/// The result of [`fold_into_last_user_message`].
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Folded {
    /// The conversation, with the text folded in.
    pub(crate) messages: Vec<Message>,
    /// How many trailing blocks of the last message are per-call content: 1 when the fold landed
    /// on the last message, otherwise 0.
    pub(crate) appended: usize,
}

/// Appends `text` as a text block to the most recent user message, which keeps its tracking id
/// and metadata (TS `foldIntoLastUserMessage`). Without a user message nothing changes.
pub(crate) fn fold_into_last_user_message(mut messages: Vec<Message>, text: &str) -> Folded {
    let Some(index) = messages.iter().rposition(|message| message.role() == Role::User) else {
        return Folded {
            messages,
            appended: 0,
        };
    };
    let target = &messages[index];
    // Some providers concatenate adjacent text blocks, which would run this onto the user's words.
    let separator = if !target.content().is_empty() && !text.starts_with('\n') {
        "\n\n"
    } else {
        ""
    };
    let mut content = target.content().to_vec();
    content.push(TextBlock::new(format!("{separator}{text}")).into());
    let mut folded = Message::new(target.role(), content).with_tracking_id(target.tracking_id());
    if let Some(metadata) = target.metadata() {
        folded = folded.with_metadata(metadata.clone());
    }
    let appended = usize::from(index == messages.len() - 1);
    messages[index] = folded;
    Folded { messages, appended }
}

#[cfg(test)]
mod tests;
