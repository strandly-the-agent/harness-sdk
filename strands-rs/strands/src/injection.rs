//! Context injection (TS `injection/`): the configuration types shared by the `ContextInjector`
//! plugin and the memory manager's injection, and the crate-internal engine that folds text into
//! the latest user message for one model call.

#![allow(
    dead_code,
    unused_imports,
    reason = "the fold engine and the XML helpers are used by the memory manager (Phase 6)"
)]

mod message_injection;
mod types;
mod xml;

pub(crate) use message_injection::{
    Folded, apply_injection, create_injection_middleware, fold_into_last_user_message, is_user_turn,
};
pub use types::{InjectionConfig, InjectionContext, InjectionTrigger};
pub(crate) use types::{InjectionMiddlewareOptions, RenderContent, Trigger, render_content};
pub(crate) use xml::{escape_xml_attr, escape_xml_text};
