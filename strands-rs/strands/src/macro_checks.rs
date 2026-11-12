//! Compile checks for `#[tool]` and `#[tools]`, run as doctests so they reuse the build.
//!
//! A downstream crate under strict lints (the macros must expand warning-free):
//!
//! ```
//! #![deny(
//!     warnings,
//!     missing_docs,
//!     clippy::pedantic,
//!     clippy::unwrap_used,
//!     clippy::expect_used,
//!     clippy::allow_attributes_without_reason
//! )]
//! //! A strict downstream crate.
//!
//! use std::sync::Arc;
//!
//! use strands::hooks::BeforeToolCallEvent;
//! use strands::{Agent, ToolContext, ToolError, tool, tool_list, tools};
//!
//! /// Get the current weather for a city.
//! #[tool]
//! pub async fn get_weather(
//!     /// City name, for example "Seattle".
//!     city: String,
//!     #[tool(context)] ctx: &ToolContext,
//! ) -> Result<String, ToolError> {
//!     Ok(format!("Sunny in {city} ({})", ctx.tool_use().tool_use_id))
//! }
//!
//! /// Adds two numbers.
//! #[tool(name = "add-numbers")]
//! #[must_use]
//! pub fn add(a: f64, b: f64) -> f64 {
//!     a + b
//! }
//!
//! /// An inventory.
//! pub struct Inventory {
//!     stock: u32,
//! }
//!
//! #[tools]
//! impl Inventory {
//!     /// Look up stock for a product.
//!     #[tool]
//!     async fn check_stock(&self, sku: String) -> Result<u32, ToolError> {
//!         Ok(if sku.is_empty() { 0 } else { self.stock })
//!     }
//! }
//!
//! fn main() -> Result<(), strands::Error> {
//!     let inventory = Arc::new(Inventory { stock: 3 });
//!     let agent = Agent::builder()
//!         .printer(false)
//!         .tools(tool_list![GetWeather, Add, inventory.tools()])
//!         .build()?;
//!     agent.add_hook(|event: &mut BeforeToolCallEvent| {
//!         if event.tool_use().name == "shell" {
//!             event.set_cancel("shell is disabled");
//!         }
//!     });
//!     agent.add_async_hook::<BeforeToolCallEvent>(|event| {
//!         Box::pin(async move {
//!             let _name = event.tool_use().name.clone();
//!         })
//!     });
//!     let _ = add(1.0, 2.0);
//!     Ok(())
//! }
//! ```
//!
//! A missing doc comment is an error, because TS rejects a tool with an empty description:
//!
//! ```compile_fail
//! #[strands::tool]
//! async fn undocumented(city: String) -> String {
//!     city
//! }
//! ```
//!
//! Two context parameters:
//!
//! ```compile_fail
//! use strands::ToolContext;
//!
//! /// Two contexts.
//! #[strands::tool]
//! async fn twice(#[tool(context)] a: &ToolContext, #[tool(context)] b: &ToolContext) -> String {
//!     format!("{}{}", a.tool_use().name, b.tool_use().name)
//! }
//! ```
//!
//! `self` by value:
//!
//! ```compile_fail
//! struct Inventory;
//!
//! #[strands::tools]
//! impl Inventory {
//!     /// Takes self by value.
//!     #[tool]
//!     async fn consume(self) -> String {
//!         String::new()
//!     }
//! }
//! ```
//!
//! A generic function:
//!
//! ```compile_fail
//! /// Generic.
//! #[strands::tool]
//! async fn generic<T: Default>(name: String) -> String {
//!     let _ = T::default();
//!     name
//! }
//! ```
//!
//! An invalid name:
//!
//! ```compile_fail
//! /// Bad name.
//! #[strands::tool(name = "has space")]
//! async fn named() -> String {
//!     String::new()
//! }
//! ```
//!
//! A return value that isn't serializable:
//!
//! ```compile_fail
//! struct Opaque;
//!
//! /// Returns something that can't be serialized.
//! #[strands::tool]
//! async fn opaque() -> Opaque {
//!     Opaque
//! }
//! ```
//!
//! The same cases compile once fixed, so each failure above is the one named:
//!
//! ```
//! use strands::ToolContext;
//!
//! /// Documented.
//! #[strands::tool]
//! async fn documented(city: String) -> String { city }
//!
//! /// One context.
//! #[strands::tool(name = "has-dash")]
//! async fn once(#[tool(context)] a: &ToolContext) -> String { a.tool_use().name.clone() }
//!
//! struct Inventory;
//!
//! #[strands::tools]
//! impl Inventory {
//!     /// Borrows self.
//!     #[tool]
//!     async fn borrow(&self) -> String { String::new() }
//! }
//! ```
