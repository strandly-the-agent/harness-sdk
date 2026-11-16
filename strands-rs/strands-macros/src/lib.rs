//! Procedural macros for the Strands Agents Rust SDK.
//!
//! - [`macro@tool`] turns a function into a tool.
//! - [`macro@tools`] turns the `#[tool]` methods of an impl block into tools that share the
//!   value's state.
//!
//! Use them through the `strands` crate (`strands::tool`, `strands::tools`), not directly: the
//! generated code names `strands` types.

mod args;
mod expand;
mod signature;

use darling::FromMeta;
use darling::ast::NestedMeta;
use proc_macro::TokenStream;
use syn::{ItemFn, ItemImpl, parse_macro_input};

use crate::args::{ToolArgs, ToolsArgs};

/// Turns a function into a tool.
///
/// The function stays as written. The macro adds a unit struct named after it in `PascalCase`
/// (`get_weather` becomes `GetWeather`) that implements `strands::Tool`, with the function's
/// visibility and doc comment:
///
/// - The tool name is the function name; `#[tool(name = "..")]` overrides it.
/// - Generated code names the `strands` crate as `::strands`; under a renamed dependency, pass
///   its path with `#[tool(crate = "::my_strands")]` (`#[tools(crate = "..")]` on an impl block).
/// - The doc comment is the tool description, and is required. Parameter doc comments are the
///   property descriptions, and `#[serde(..)]` and `#[schemars(..)]` attributes on parameters
///   shape the input schema.
/// - Parameters are owned, deserializable types. The input is validated against the derived
///   schema before the call; a failure is an error result.
/// - One parameter may be marked `#[tool(context)]` to receive the `ToolContext` (by reference
///   or by value).
/// - The return type is `Result<T, E>` with `T: Serialize` and `E: Into<ToolError>`, or any
///   serializable value. An `Err` becomes an error result, and `ctx.interrupt(..)?` ends the tool
///   with an interrupt. Every `std::error::Error` converts; a boxed error or `anyhow::Error`
///   does not, so return `Result<T, ToolError>` and write
///   `.map_err(ToolError::from_boxed)?`.
/// - An `async fn` runs on the tool task; a sync fn runs on a blocking thread.
/// - The generated struct is the tool: pass it to `Agent::builder().tool(GetWeather)`, or
///   several of different types as `.tools(strands::tool_list![GetWeather, GetTime])`.
///
/// ```
/// use strands::tool;
///
/// /// Get the current weather for a city.
/// #[tool]
/// async fn get_weather(
///     /// City name, for example "Seattle".
///     city: String,
/// ) -> Result<String, strands::ToolError> {
///     Ok(format!("Sunny in {city}"))
/// }
///
/// use strands::Tool;
/// assert_eq!(GetWeather.name(), "get_weather");
/// assert_eq!(GetWeather.description(), "Get the current weather for a city.");
/// ```
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemFn);
    let expanded = NestedMeta::parse_meta_list(attr.into())
        .map_err(darling::Error::from)
        .and_then(|list| ToolArgs::from_list(&list))
        .map_err(syn::Error::from)
        .and_then(|args| expand::tool_fn(args, item));
    expanded.unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Turns the `#[tool]` methods of an impl block into tools bound to an `Arc` of the value.
///
/// Each `#[tool]` method takes `&self` and follows the `#[tool]` rules. The block gains
/// `<method>_tool(self: &Arc<Self>)`, returning that tool, and `tools(self: &Arc<Self>)`,
/// returning all of them as a `Vec<ToolSource>` in declaration order.
///
/// - `tools()` is public when any `#[tool]` method is, and otherwise has the visibility of the
///   first one; each `<method>_tool` has its method's visibility.
/// - Every block generates the list method, so a type has one `#[tools]` block, or each further
///   block renames its list: `#[tools(list = "admin_tools")]`. A `#[tool]` method can't have
///   the list method's name, and `<method>_tool` can't be the name of another method of the block.
/// - A `#[tool]` method under `#[cfg(..)]` takes its tool, its constructor and its `tools()`
///   entry with it.
/// - `strands::tools` is both a module and this attribute. In a crate with its own `tools`
///   module, write `#[strands::tools]` instead of importing it.
///
/// ```
/// use std::sync::Arc;
///
/// use strands::{Tool, ToolContext, ToolError, tools};
///
/// struct Inventory {
///     stock: u32,
/// }
///
/// #[tools]
/// impl Inventory {
///     /// Look up stock for a product.
///     #[tool]
///     async fn check_stock(
///         &self,
///         #[tool(context)] ctx: &ToolContext,
///         /// Product SKU.
///         sku: String,
///     ) -> Result<u32, ToolError> {
///         let _ = (ctx.tool_use(), sku);
///         Ok(self.stock)
///     }
/// }
///
/// let inventory = Arc::new(Inventory { stock: 3 });
/// assert_eq!(inventory.check_stock_tool().name(), "check_stock");
/// assert_eq!(inventory.tools().len(), 1);
/// ```
#[proc_macro_attribute]
pub fn tools(attr: TokenStream, item: TokenStream) -> TokenStream {
    let item = parse_macro_input!(item as ItemImpl);
    let expanded = NestedMeta::parse_meta_list(attr.into())
        .map_err(darling::Error::from)
        .and_then(|list| ToolsArgs::from_list(&list))
        .map_err(syn::Error::from)
        .and_then(|args| expand::tools_impl(&args, item));
    expanded.unwrap_or_else(syn::Error::into_compile_error).into()
}
