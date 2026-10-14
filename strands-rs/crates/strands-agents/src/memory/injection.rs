//! Memory injection (`strands-ts/src/memory/memory-manager.ts` injection internals +
//! `strands-ts/src/injection/xml.ts`): folds retrieved entries into the model input for one call.
//!
//! TS uses an `InvokeModelStage.Input` middleware; Rust uses the equivalent seam,
//! [`BeforeModelCallEvent::messages`](crate::hooks::BeforeModelCallEvent) — see
//! [`ContextInjector`](crate::plugins::ContextInjector), whose `is_user_turn` /
//! `fold_into_last_user_message` primitives this module reuses.

use std::sync::Arc;

use crate::memory::types::MemoryEntry;
use crate::plugins::vended::context_injector::is_user_turn;
use crate::types::{ContentBlock, Message, Role};

/// Entries injected per model call when the config does not say.
pub const DEFAULT_MAX_ENTRIES: usize = 5;

/// Renders retrieved entries into the injected text.
pub type InjectionFormatFn = Arc<dyn Fn(&[MemoryEntry]) -> String + Send + Sync>;
/// Derives the search query from the conversation; `None` skips injection for this call.
pub type InjectionQueryFn = Arc<dyn Fn(&[Message]) -> Option<String> + Send + Sync>;

/// When and what [`MemoryManager`](crate::memory::MemoryManager) injects before a model call.
#[derive(Clone)]
pub struct MemoryInjectionConfig {
    /// When injection runs. Defaults to a fresh user ask only.
    pub trigger: InjectionTrigger,
    /// Cap on retrieved + injected entries ([`DEFAULT_MAX_ENTRIES`] when `None`).
    pub max_entries: Option<usize>,
    /// Query override; `None` uses the adaptive default ([`default_injection_query`]).
    pub query: Option<InjectionQueryFn>,
    /// Format override; `None` uses the `<memory>` block ([`default_injection_format`]).
    pub format: Option<InjectionFormatFn>,
}

impl Default for MemoryInjectionConfig {
    fn default() -> Self {
        Self {
            trigger: InjectionTrigger::UserTurn,
            max_entries: None,
            query: None,
            format: None,
        }
    }
}

impl std::fmt::Debug for MemoryInjectionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MemoryInjectionConfig")
            .field("trigger", &self.trigger)
            .field("max_entries", &self.max_entries)
            .field("query", &self.query.is_some())
            .field("format", &self.format.is_some())
            .finish()
    }
}

impl MemoryInjectionConfig {
    /// Resolved entry cap.
    pub fn max_entries(&self) -> usize {
        self.max_entries.unwrap_or(DEFAULT_MAX_ENTRIES).max(1)
    }

    /// Whether this call should inject.
    pub fn should_inject(&self, messages: &[Message]) -> bool {
        match self.trigger {
            InjectionTrigger::EveryTurn => true,
            InjectionTrigger::UserTurn => is_user_turn(messages),
        }
    }

    /// Query for this call: the configured callback, else [`default_injection_query`].
    pub fn query(&self, messages: &[Message]) -> Option<String> {
        let query = match &self.query {
            Some(custom) => custom(messages),
            None => default_injection_query(messages),
        }?;
        let query = query.trim().to_owned();
        (!query.is_empty()).then_some(query)
    }

    /// Injected text for `entries`: the configured formatter, else [`default_injection_format`].
    pub fn format(&self, entries: &[MemoryEntry]) -> String {
        match &self.format {
            Some(custom) => custom(entries),
            None => default_injection_format(entries),
        }
    }
}

pub use crate::plugins::vended::context_injector::InjectionTrigger;

/// Adaptive default query: the latest user text on a user turn, else the most recent assistant
/// text (the previous step of an autonomous turn).
pub fn default_injection_query(messages: &[Message]) -> Option<String> {
    let role = if is_user_turn(messages) {
        Role::User
    } else {
        Role::Assistant
    };
    let message = messages.iter().rev().find(|m| m.role == role)?;
    let text: Vec<&str> = message
        .content
        .iter()
        .filter_map(|block| match block {
            ContentBlock::Text(text) => Some(text.as_str()),
            _ => None,
        })
        .collect();
    let joined = text.join("\n").trim().to_owned();
    (!joined.is_empty()).then_some(joined)
}

/// Default format: a `<memory>` block with one escaped `<entry>` per result, carrying a `source`
/// attribute when the originating store is known.
pub fn default_injection_format(entries: &[MemoryEntry]) -> String {
    let items: Vec<String> = entries
        .iter()
        .map(|entry| match &entry.store_name {
            Some(store) => format!(
                "<entry source=\"{}\">{}</entry>",
                escape_xml_attr(store),
                escape_xml_text(&entry.content)
            ),
            None => format!("<entry>{}</entry>", escape_xml_text(&entry.content)),
        })
        .collect();
    format!("<memory>\n{}\n</memory>", items.join("\n"))
}

/// Escapes text for placement between XML tags (`&` first, so later replacements are not
/// double-escaped).
pub fn escape_xml_text(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// [`escape_xml_text`] plus the quote characters, for a double-quoted XML attribute.
pub fn escape_xml_attr(value: &str) -> String {
    escape_xml_text(value)
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn user(text: &str) -> Message {
        Message::new(Role::User, vec![ContentBlock::text(text)])
    }

    fn assistant(text: &str) -> Message {
        Message::new(Role::Assistant, vec![ContentBlock::text(text)])
    }

    #[test]
    fn escapes_text_and_attributes() {
        assert_eq!(escape_xml_text("a & b <c> d"), "a &amp; b &lt;c&gt; d");
        assert_eq!(escape_xml_attr("\"x\" & 'y'"), "&quot;x&quot; &amp; &#39;y&#39;");
    }

    #[test]
    fn default_format_emits_a_memory_block_with_sources() {
        let entries = vec![
            MemoryEntry {
                content: "likes <tea>".to_owned(),
                store_name: Some("store \"a\"".to_owned()),
                metadata: None,
            },
            MemoryEntry::new("no source"),
        ];
        assert_eq!(
            default_injection_format(&entries),
            "<memory>\n<entry source=\"store &quot;a&quot;\">likes &lt;tea&gt;</entry>\n<entry>no source</entry>\n</memory>"
        );
    }

    #[test]
    fn default_query_uses_the_last_user_text_on_a_user_turn() {
        let messages = vec![user("first"), assistant("reply"), user("second ask")];
        assert_eq!(default_injection_query(&messages), Some("second ask".to_owned()));
    }

    #[test]
    fn default_query_falls_back_to_the_last_assistant_text_off_turn() {
        let messages = vec![user("ask"), assistant("thinking about rust")];
        assert_eq!(
            default_injection_query(&messages),
            Some("thinking about rust".to_owned())
        );
    }

    #[test]
    fn default_query_is_none_without_matching_text() {
        assert_eq!(default_injection_query(&[]), None);
        assert_eq!(default_injection_query(&[user("   "), assistant("")]), None);
    }

    #[test]
    fn user_turn_trigger_skips_assistant_turns() {
        let config = MemoryInjectionConfig::default();
        assert!(config.should_inject(&[user("hi")]));
        assert!(!config.should_inject(&[assistant("hi")]));
        let every = MemoryInjectionConfig {
            trigger: InjectionTrigger::EveryTurn,
            ..Default::default()
        };
        assert!(every.should_inject(&[assistant("hi")]));
    }

    #[test]
    fn custom_query_and_format_win() {
        let config = MemoryInjectionConfig {
            query: Some(Arc::new(|_| Some("  fixed  ".to_owned()))),
            format: Some(Arc::new(|entries| format!("{} entries", entries.len()))),
            ..Default::default()
        };
        assert_eq!(config.query(&[user("hi")]), Some("fixed".to_owned()));
        assert_eq!(config.format(&[MemoryEntry::new("x")]), "1 entries");
    }

    #[test]
    fn max_entries_defaults_and_floors_at_one() {
        assert_eq!(MemoryInjectionConfig::default().max_entries(), 5);
        assert_eq!(
            MemoryInjectionConfig {
                max_entries: Some(0),
                ..Default::default()
            }
            .max_entries(),
            1
        );
    }
}
