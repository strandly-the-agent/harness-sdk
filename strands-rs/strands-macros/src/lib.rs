//! Procedural macros for the Strands Agents Rust SDK.
//!
//! Use them through the `strands` crate (`strands::tool`, `strands::tools`), not directly.

mod tool;
mod tools;

use proc_macro::TokenStream;

/// Turns a function into a tool.
///
/// Keeps the function and adds a unit struct named after it in `PascalCase` that implements
/// `strands::Tool`. The doc comment is the description, parameter doc comments describe the
/// input properties, `#[tool(name = "..")]` overrides the model-facing name and
/// `#[tool(context)]` marks a `&ToolContext` parameter. See the `strands` crate docs.
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::expand(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Generates tools from the `#[tool]` methods of an impl block: `X::<method>_tool(&arc)` and
/// `X::tools(&arc)`. See the `strands` crate docs.
#[proc_macro_attribute]
pub fn tools(attr: TokenStream, item: TokenStream) -> TokenStream {
    tools::expand(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
