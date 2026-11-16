//! The token-usage middleware (TS `createTokenUsageMiddleware` in
//! `context-manager/modes/agentic/agentic-context.ts`): tells the model how much of its context
//! window the call uses. The agentic context manager installs it (Phase 6).

use crate::middleware::{InvokeModelContext, InvokeModelStage, Middleware, MiddlewareStage};
use crate::models::model_info::DEFAULT_CONTEXT_WINDOW_LIMIT;
use crate::types::messages::{Message, Role, TextBlock};

/// `value` with en-US thousands separators (TS `toLocaleString('en-US')`).
fn grouped(value: u64) -> String {
    let digits = value.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `used / limit` as a percentage with one decimal, ties rounded up like TS `toFixed(1)`.
fn percent(used: u64, limit: u64) -> String {
    let tenths = (u128::from(used) * 2000 + u128::from(limit)) / (2 * u128::from(limit));
    format!("{}.{}", tenths / 10, tenths % 10)
}

/// Appends a `<context-status>` block to the last message of the call's copy, when the projected
/// input is known; it counts as a per-call trailing block only on a user message.
pub(crate) fn add_context_status(mut context: InvokeModelContext) -> InvokeModelContext {
    let Some(used) = context.projected_input_tokens else {
        return context;
    };
    let limit = context
        .model
        .model_info()
        .context_window()
        .filter(|&limit| limit > 0)
        .unwrap_or(DEFAULT_CONTEXT_WINDOW_LIMIT);
    let status = format!(
        "\n\n<context-status>\n<used>{} / {} tokens ({}%)</used>\n<remaining>~{} tokens</remaining>\n</context-status>",
        grouped(used),
        grouped(limit),
        percent(used, limit),
        grouped(limit.saturating_sub(used)),
    );
    let Some(last) = context.messages.last_mut() else {
        return context;
    };
    let role = last.role();
    let mut content = last.content().to_vec();
    content.push(TextBlock::new(status).into());
    let mut message = Message::new(role, content);
    if let Some(metadata) = last.metadata() {
        message = message.with_metadata(metadata.clone());
    }
    *last = message;
    if role == Role::User {
        context.dynamic_trailing_blocks = Some(context.dynamic_trailing_blocks.unwrap_or(0) + 1);
    }
    context
}

/// The middleware as an `InvokeModelStage` Input handler.
#[cfg_attr(
    not(test),
    allow(dead_code, reason = "installed by the agentic context manager (Phase 6)")
)]
pub(crate) fn token_usage_middleware() -> Middleware<InvokeModelStage> {
    InvokeModelStage::input(|context| async move { Ok(add_context_status(context)) })
}

#[cfg(test)]
mod tests;
