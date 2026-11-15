//! HTML to markdown extraction for the web fetch tool (TS `vended-tools/web-fetch/extract.ts`).
//!
//! ATX headings and fenced code blocks. Non-content elements (scripts, styles, media, forms and
//! so on) are dropped with their subtree, `data:` images become their alt text, and
//! `javascript:`/`vbscript:` sources and links are stripped.

use std::sync::{LazyLock, OnceLock};

use htmd::element_handler::{HandlerResult, Handlers};
use htmd::options::{BulletListMarker, CodeBlockStyle, HeadingStyle, LinkStyle, Options};
use htmd::{Element, HtmlToMarkdown};
use regex::Regex;

/// Elements whose entire subtree is discarded.
const DROPPED_ELEMENTS: [&str; 17] = [
    "script", "style", "noscript", "template", "svg", "canvas", "iframe", "object", "embed", "video",
    "audio", "form", "input", "button", "select", "textarea", "nav",
];

static LEADING_INVISIBLE: LazyLock<Option<Regex>> =
    LazyLock::new(|| Regex::new(r"^[\p{Cc}\p{Cf}\p{Zs}]+").ok());

/// The configured converter, built once (TS `_turndownServicePromise`).
static CONVERTER: OnceLock<HtmlToMarkdown> = OnceLock::new();

/// Converts HTML to markdown for a model to read (TS `htmlToMarkdown`): the page title becomes
/// a top-level heading, and the result is trimmed and ends with one newline (empty when there is
/// no content). Returns `html` unchanged if conversion fails.
pub(crate) fn html_to_markdown(html: &str) -> String {
    match CONVERTER.get_or_init(converter).convert(html) {
        Ok(markdown) => {
            let markdown = markdown.trim();
            if markdown.is_empty() {
                String::new()
            } else {
                format!("{markdown}\n")
            }
        }
        Err(error) => {
            tracing::warn!(error = %error, "html_to_markdown failed, returning raw html");
            html.to_owned()
        }
    }
}

fn converter() -> HtmlToMarkdown {
    HtmlToMarkdown::builder()
        .options(Options {
            heading_style: HeadingStyle::Atx,
            code_block_style: CodeBlockStyle::Fenced,
            bullet_list_marker: BulletListMarker::Dash,
            link_style: LinkStyle::Inlined,
            ..Options::default()
        })
        .skip_tags(DROPPED_ELEMENTS.to_vec())
        .add_handler(vec!["title"], |handlers: &dyn Handlers, element: Element<'_>| {
            title(handlers, &element)
        })
        .add_handler(vec!["img"], |_: &dyn Handlers, element: Element<'_>| {
            safe_image(&element)
        })
        .add_handler(vec!["a"], safe_link)
        .build()
}

/// The `<title>` as a top-level heading, so it appears in the body (turndown drops `<head>`).
fn title(handlers: &dyn Handlers, element: &Element<'_>) -> Option<HandlerResult> {
    let content = handlers.walk_children(element.node).content;
    let text = content.trim();
    Some(
        if text.is_empty() {
            String::new()
        } else {
            format!("# {text}\n\n")
        }
        .into(),
    )
}

/// `data:` images become their alt text; `javascript:` and `vbscript:` sources are dropped.
fn safe_image(element: &Element<'_>) -> Option<HandlerResult> {
    let src = attribute(element, "src").trim();
    let alt = attribute(element, "alt");
    if src.is_empty() {
        return Some(alt.into());
    }
    let rendered = match scheme(src).as_deref() {
        Some("javascript:" | "vbscript:") => String::new(),
        Some("data:") => alt.to_owned(),
        _ => format!("![{alt}]({src})"),
    };
    Some(rendered.into())
}

/// Only the link text for `javascript:`, `vbscript:` and `data:` hrefs.
fn safe_link(handlers: &dyn Handlers, element: Element<'_>) -> Option<HandlerResult> {
    let href = attribute(&element, "href").trim();
    if matches!(
        scheme(href).as_deref(),
        Some("javascript:" | "vbscript:" | "data:")
    ) {
        return Some(handlers.walk_children(element.node));
    }
    handlers.fallback(element)
}

fn attribute<'a>(element: &'a Element<'_>, name: &str) -> &'a str {
    element
        .attrs
        .iter()
        .find(|attribute| &*attribute.name.local == name)
        .map_or("", |attribute| &*attribute.value)
}

/// The URL scheme with its colon (TS `new URL(..).protocol`), after stripping leading invisible
/// characters; `None` for a value that isn't an absolute URL.
fn scheme(value: &str) -> Option<String> {
    let stripped = LEADING_INVISIBLE.as_ref().map_or(value, |pattern| {
        pattern
            .find(value)
            .and_then(|found| value.get(found.end()..))
            .unwrap_or(value)
    });
    reqwest::Url::parse(stripped)
        .ok()
        .map(|url| format!("{}:", url.scheme()))
}

#[cfg(test)]
mod tests;
