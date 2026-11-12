//! Procedural macros for the Strands Agents Rust SDK.
//!
//! Use them through the `strands` crate (`strands::tool`, `strands::tools`), not directly.

use proc_macro::TokenStream;

/// Turns a function into a tool.
#[proc_macro_attribute]
pub fn tool(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

/// Generates tools from an impl block's `#[tool]` methods.
#[proc_macro_attribute]
pub fn tools(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}
