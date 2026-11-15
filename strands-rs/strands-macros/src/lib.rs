//! Procedural macros for the Strands Agents Rust SDK.
//!
//! Use them through the `strands` crate (`strands::tool`, `strands::tools`) rather than depending on
//! this crate directly. Generated code names its dependencies through `strands`, so a crate using
//! the macros needs only `strands`.

mod tool;
mod tools;

use proc_macro::TokenStream;

/// Turns a function into a tool.
///
/// The function is kept, and a unit struct named after it in `PascalCase` (`get_weather` becomes
/// `GetWeather`) implements `strands::Tool`. The struct has the function's visibility and doc
/// comment.
///
/// - The doc comment is the tool description and is required. Parameter doc comments describe
///   the input properties.
/// - The model calls the tool by the function name; `#[tool(name = "...")]` overrides it. Names
///   are 1 to 64 ASCII letters, digits, `_` or `-`.
/// - Parameters are owned types implementing `serde::Deserialize` and `schemars::JsonSchema`; the
///   input schema is derived from them and input is validated against it. `Option<T>` parameters
///   are optional.
/// - Mark one parameter `#[tool(context)]` to receive the `strands::ToolContext`, by reference or
///   by value.
/// - Return `Result<T, E>` with `T: Serialize` and `E: Into<strands::ToolError>`, or a plain
///   serializable value. An `Err` becomes an error result; `ctx.interrupt(..)?` pauses the call.
/// - A synchronous function runs on Tokio's blocking pool.
///
/// ```
/// use strands::{ToolContext, ToolError, tool};
///
/// /// Get the current weather for a city.
/// #[tool]
/// async fn get_weather(
///     /// City name, for example "Seattle".
///     city: String,
/// ) -> Result<String, ToolError> {
///     Ok(format!("Sunny in {city}"))
/// }
///
/// /// Ask the user before deleting a file.
/// #[tool(name = "delete-file")]
/// fn delete_file(#[tool(context)] ctx: &ToolContext, path: String) -> Result<String, ToolError> {
///     let answer = ctx.interrupt("confirm")?;
///     Ok(format!("{path}: {answer}"))
/// }
///
/// let tools = strands::tool_list![GetWeather, DeleteFile];
/// # assert_eq!(tools.len(), 2);
/// ```
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    tool::expand(attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}

/// Turns the `#[tool]` methods of an `impl` block into tools.
///
/// Each `#[tool]` method takes `&self` and follows the `#[tool]` rules for functions. For a method
/// `check_stock` the block gains `check_stock_tool(self: &Arc<Self>) -> Arc<dyn Tool>`, and the
/// block gains `tools(self: &Arc<Self>) -> Vec<ToolSource>` with every tool in order. The tools
/// share the `Arc`.
///
/// ```
/// use std::sync::Arc;
/// use strands::{ToolContext, ToolError, tools};
///
/// /// A product inventory.
/// pub struct Inventory {
///     stock: u32,
/// }
///
/// #[tools]
/// impl Inventory {
///     /// Look up stock for a product.
///     #[tool]
///     pub async fn check_stock(&self, #[tool(context)] ctx: &ToolContext, sku: String) -> Result<u32, ToolError> {
///         let _region = ctx.agent().app_state().get::<String>("region")?;
///         Ok(if sku.is_empty() { 0 } else { self.stock })
///     }
/// }
///
/// let inventory = Arc::new(Inventory { stock: 3 });
/// assert_eq!(inventory.tools().len(), 1);
/// ```
#[proc_macro_attribute]
pub fn tools(attr: TokenStream, item: TokenStream) -> TokenStream {
    tools::expand(&attr.into(), item.into())
        .unwrap_or_else(syn::Error::into_compile_error)
        .into()
}
