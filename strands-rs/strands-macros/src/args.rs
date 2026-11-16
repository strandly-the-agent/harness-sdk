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
    /// The path of the `strands` crate, for a renamed dependency.
    #[darling(rename = "crate")]
    pub(crate) krate: Option<syn::Path>,
}

/// Arguments of `#[tools(..)]` on an impl block.
#[derive(Debug, Default, FromMeta)]
#[darling(default)]
pub(crate) struct ToolsArgs {
    /// The path of the `strands` crate, for a renamed dependency.
    #[darling(rename = "crate")]
    pub(crate) krate: Option<syn::Path>,
    /// The name of the generated list method (default `tools`).
    pub(crate) list: Option<darling::util::SpannedValue<String>>,
}

/// The `strands` crate path: `krate`, or `::strands`.
pub(crate) fn crate_path(krate: Option<&syn::Path>) -> syn::Path {
    krate.cloned().unwrap_or_else(|| syn::parse_quote!(::strands))
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
