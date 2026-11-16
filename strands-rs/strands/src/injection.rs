//! Context injection (TS `injection/`): the configuration types shared by the `ContextInjector`
//! plugin and the memory manager's injection, and the crate-internal engine that folds text into
//! the latest user message for one model call.

mod message_injection;
mod types;
mod xml;

pub(crate) use message_injection::{create_injection_middleware, is_user_turn};
pub use types::{
    InjectionCondition, InjectionConfig, InjectionConfigBuilder, InjectionContext, InjectionTrigger,
    injection_config_builder,
};
pub(crate) use types::{InjectionMiddlewareOptions, RenderContent, render_content};
pub(crate) use xml::{escape_xml_attr, escape_xml_text};
