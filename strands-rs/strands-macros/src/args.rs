//! The `#[tool(..)]` attribute arguments.
#![expect(
    clippy::unreachable,
    reason = "darling's `FromMeta` derive expands to `unreachable!`"
)]

use darling::FromMeta;
use darling::util::Flag;
use syn::{Attribute, Meta};

/// Arguments of `#[tool(..)]` on a function or method.
#[derive(Debug, Default, FromMeta)]
#[darling(default)]
pub(crate) struct ToolArgs {
    pub(crate) name: Option<String>,
}

impl ToolArgs {
    /// Parses an inner `#[tool]` or `#[tool(..)]` attribute.
    pub(crate) fn from_attr(attr: &Attribute) -> syn::Result<Self> {
        match &attr.meta {
            Meta::Path(_) => Ok(Self::default()),
            meta => Ok(Self::from_meta(meta)?),
        }
    }
}

/// Arguments of `#[tool(..)]` on a parameter.
#[derive(Debug, FromMeta)]
pub(crate) struct ParamArgs {
    pub(crate) context: Flag,
}
