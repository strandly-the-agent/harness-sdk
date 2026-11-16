//! Message injection for memory (TS `injection/message-injection.ts`, `injection/types.ts` and
//! `injection/xml.ts`): the trigger types and the fold into the last user message.

use std::fmt;
use std::future::Future;
use std::sync::Arc;

use crate::error::Result;
use crate::middleware::{InvokeModelContext, InvokeModelStage, Middleware, MiddlewareStage};
use crate::types::agent::ToolAgent;
use crate::types::messages::{ContentBlock, Message, Role, TextBlock};

/// What an injection trigger or renderer sees (TS `InjectionContext`).
#[derive(Debug)]
#[non_exhaustive]
pub struct InjectionContext<'a> {
    /// The messages about to be sent to the model.
    pub messages: &'a [Message],
    /// The agent; its `app_state` is TS `appState`.
    pub agent: &'a ToolAgent,
}

/// A custom trigger predicate; an error skips injection.
pub type InjectionPredicate = Arc<dyn Fn(&InjectionContext<'_>) -> Result<bool> + Send + Sync>;

/// When injected content is added (TS `InjectionTrigger`, or a predicate).
#[derive(Clone, Default)]
#[non_exhaustive]
pub enum InjectionTrigger {
    /// Only on a fresh user ask (TS `'userTurn'`).
    #[default]
    UserTurn,
    /// On every model call (TS `'everyTurn'`).
    EveryTurn,
    /// When the predicate returns `true`.
    Predicate(InjectionPredicate),
}

impl fmt::Debug for InjectionTrigger {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UserTurn => "UserTurn",
            Self::EveryTurn => "EveryTurn",
            Self::Predicate(_) => "Predicate(..)",
        })
    }
}

impl InjectionTrigger {
    fn fires(&self, context: &InjectionContext<'_>) -> bool {
        match self {
            Self::UserTurn => is_user_turn(context.messages),
            Self::EveryTurn => true,
            Self::Predicate(predicate) => predicate(context).unwrap_or_else(|error| {
                tracing::warn!("reason=<{error}> | injection trigger threw; skipping injection");
                false
            }),
        }
    }
}

/// An `InvokeModelStage` Input middleware that folds `render_content`'s text into the last user
/// message when `trigger` fires (TS `createInjectionMiddleware`). Fails open.
pub(crate) fn create_injection_middleware<F, Fut>(
    trigger: Option<InjectionTrigger>,
    render_content: F,
) -> Middleware<InvokeModelStage>
where
    F: Fn(Vec<Message>) -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Result<Option<String>>> + Send + 'static,
{
    let trigger = trigger.unwrap_or_default();
    let render_content = Arc::new(render_content);
    InvokeModelStage::input(move |mut context: InvokeModelContext| {
        let fires = trigger.fires(&InjectionContext {
            messages: &context.messages,
            agent: context.agent(),
        });
        let render_content = Arc::clone(&render_content);
        async move {
            if !fires {
                return Ok(context);
            }
            let text = match render_content(context.messages.clone()).await {
                Ok(text) => text,
                Err(error) => {
                    tracing::warn!("reason=<{error}> | injection renderContent threw; skipping injection");
                    return Ok(context);
                }
            };
            let Some(text) = text.filter(|text| !text.trim().is_empty()) else {
                return Ok(context);
            };
            let appended = fold_into_last_user_message(&mut context.messages, &text);
            context.dynamic_trailing_blocks = Some(
                context
                    .dynamic_trailing_blocks
                    .unwrap_or(0)
                    .saturating_add(appended),
            );
            Ok(context)
        }
    })
}

/// Whether the last message is a fresh user ask rather than tool results (TS `isUserTurn`).
pub(crate) fn is_user_turn(messages: &[Message]) -> bool {
    messages.last().is_some_and(|last| {
        last.role() == Role::User
            && !last
                .content()
                .iter()
                .any(|block| matches!(block, ContentBlock::ToolResult(_)))
    })
}

/// Appends `text` as a text block to the last user message (TS `foldIntoLastUserMessage`), and
/// returns how many trailing blocks that added to the final message.
pub(crate) fn fold_into_last_user_message(messages: &mut [Message], text: &str) -> usize {
    let last_index = messages.len().saturating_sub(1);
    let Some((index, target)) = messages
        .iter_mut()
        .enumerate()
        .rev()
        .find(|(_, message)| message.role() == Role::User)
    else {
        return 0;
    };
    let separator = if !target.content().is_empty() && !text.starts_with('\n') {
        "\n\n"
    } else {
        ""
    };
    let mut content = target.content().to_vec();
    content.push(TextBlock::new(format!("{separator}{text}")).into());
    let mut folded = Message::new(target.role(), content).with_tracking_id(target.tracking_id());
    folded.set_metadata(target.metadata().cloned());
    *target = folded;
    usize::from(index == last_index)
}

/// Escapes text content for XML (TS `escapeXmlText`).
pub(crate) fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Escapes an attribute value for XML (TS `escapeXmlAttr`).
pub(crate) fn escape_xml_attr(value: &str) -> String {
    escape_xml_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}
