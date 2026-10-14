//! Procedural macros for the Strands Agents SDK for Rust.
//!
//! Provides [`tool`], which turns a function into a type implementing
//! `strands_agents::tools::Tool`. Where the TypeScript SDK derives a tool's input schema
//! from a Zod schema at runtime, this macro derives it from the function signature at
//! compile time.

use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{
    parse_macro_input, Attribute, Expr, ExprLit, FnArg, ItemFn, Lit, Meta, Pat, ReturnType, Token, Type,
};

/// Parsed `#[tool(name = "...", description = "...")]` arguments.
#[derive(Default)]
struct ToolArgs {
    name: Option<String>,
    description: Option<String>,
}

impl Parse for ToolArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let mut args = ToolArgs::default();
        for meta in Punctuated::<Meta, Token![,]>::parse_terminated(input)? {
            let Meta::NameValue(meta) = &meta else {
                return Err(syn::Error::new(
                    meta.span(),
                    "#[tool] arguments must be `name = \"...\"` or `description = \"...\"`",
                ));
            };
            let Expr::Lit(ExprLit {
                lit: Lit::Str(value), ..
            }) = &meta.value
            else {
                return Err(syn::Error::new(meta.value.span(), "expected a string literal"));
            };
            if meta.path.is_ident("name") {
                if !is_valid_tool_name(&value.value()) {
                    return Err(syn::Error::new(
                        value.span(),
                        "tool name must be 1-64 characters of [a-zA-Z0-9_-]",
                    ));
                }
                args.name = Some(value.value());
            } else if meta.path.is_ident("description") {
                args.description = Some(value.value());
            } else {
                return Err(syn::Error::new(
                    meta.path.span(),
                    "unknown #[tool] argument; expected `name` or `description`",
                ));
            }
        }
        Ok(args)
    }
}

/// Mirrors `strands_agents::tools::is_valid_tool_name` (no dependency on the SDK crate).
fn is_valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Turns a function into a `strands_agents::tools::Tool` implementation.
///
/// The function's doc comment becomes the tool description and its parameters become the
/// JSON input schema (parameter types must implement `serde::Deserialize` and
/// `schemars::JsonSchema`); a doc comment on a parameter becomes that property's
/// `description`. A struct named `<FnNameInPascalCase>Tool` with a `new()` constructor is
/// generated next to the function.
///
/// `#[tool(name = "...", description = "...")]` overrides the tool name (1-64 characters
/// of `[a-zA-Z0-9_-]`) and the description.
///
/// A parameter of type `ToolContext` is passed through from the invocation and excluded
/// from the schema. A function returning `Result<T, E>` (`E: Display + 'static`) reports
/// `Err` as an error tool result, except for control-flow errors
/// (`Error::Interrupt` from `ToolContext::interrupt`, `Error::Cancelled`), which are
/// propagated so they still pause or cancel the event loop. Any other return type is
/// serialized as a success result.
///
/// # Example
///
/// ```ignore
/// use strands_agents::tool;
///
/// /// Get the current weather for a location.
/// #[tool]
/// async fn get_weather(
///     /// City to look up, e.g. "Seattle".
///     location: String,
/// ) -> String {
///     format!("Weather in {location}: 22C, sunny")
/// }
///
/// // Register with: Agent::builder().tool(GetWeatherTool::new())
/// ```
#[proc_macro_attribute]
pub fn tool(attr: TokenStream, item: TokenStream) -> TokenStream {
    let args = parse_macro_input!(attr as ToolArgs);
    let input_fn = parse_macro_input!(item as ItemFn);
    match expand(args, input_fn) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand(args: ToolArgs, mut input_fn: ItemFn) -> syn::Result<proc_macro2::TokenStream> {
    let fn_name = input_fn.sig.ident.clone();
    let fn_name_str = fn_name.to_string();
    let struct_name = format_ident!("{}Tool", to_pascal_case(&fn_name_str));
    let args_name = format_ident!("{}ToolArgs", to_pascal_case(&fn_name_str));
    let is_async = input_fn.sig.asyncness.is_some();
    let tool_name = args.name.unwrap_or_else(|| fn_name_str.clone());
    let description = args
        .description
        .unwrap_or_else(|| extract_doc_comment(&input_fn.attrs));

    let mut fields = Vec::new();
    let mut call_args = Vec::new();
    for arg in &mut input_fn.sig.inputs {
        let FnArg::Typed(pat_type) = arg else {
            return Err(syn::Error::new(
                arg.span(),
                "#[tool] cannot be applied to methods taking `self`",
            ));
        };
        let Pat::Ident(pat_ident) = &*pat_type.pat else {
            return Err(syn::Error::new(
                pat_type.pat.span(),
                "#[tool] parameters must be plain identifiers",
            ));
        };
        let ident = pat_ident.ident.clone();
        let docs: Vec<Attribute> = pat_type
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("doc"))
            .cloned()
            .collect();
        pat_type.attrs.retain(|attr| !attr.path().is_ident("doc"));
        let ty = &*pat_type.ty;
        if is_tool_context(ty) {
            call_args.push(quote!(ctx));
            continue;
        }
        fields.push(quote!(#(#docs)* #ident: #ty));
        call_args.push(quote!(args.#ident));
    }

    let call = if is_async {
        quote!(#fn_name(#(#call_args),*).await)
    } else {
        quote!(#fn_name(#(#call_args),*))
    };
    let output = if returns_result(&input_fn.sig.output) {
        quote! {
            let output = match #call {
                ::core::result::Result::Ok(value) => value,
                ::core::result::Result::Err(err) => {
                    return ::strands_agents::tools::tool_error_or_propagate(err, &tool_use_id);
                }
            };
        }
    } else {
        quote!(let output = #call;)
    };

    Ok(quote! {
        #input_fn

        #[doc(hidden)]
        #[derive(
            ::strands_agents::__private::serde::Deserialize,
            ::strands_agents::__private::schemars::JsonSchema,
        )]
        #[serde(crate = "::strands_agents::__private::serde")]
        #[schemars(crate = "::strands_agents::__private::schemars")]
        struct #args_name {
            #(#fields,)*
        }

        #[doc = concat!("Tool generated by `#[tool]` from `fn ", #fn_name_str, "`.")]
        #[derive(Clone, Debug)]
        pub struct #struct_name {
            spec: ::strands_agents::types::ToolSpec,
        }

        impl #struct_name {
            #[doc = concat!("Creates the `", #fn_name_str, "` tool.")]
            pub fn new() -> Self {
                Self {
                    spec: ::strands_agents::types::ToolSpec::new(#tool_name, #description)
                        .with_input_schema(
                            ::strands_agents::tools::input_schema_for::<#args_name>(),
                        ),
                }
            }
        }

        impl ::core::default::Default for #struct_name {
            fn default() -> Self {
                Self::new()
            }
        }

        #[::strands_agents::__private::async_trait]
        impl ::strands_agents::tools::Tool for #struct_name {
            fn name(&self) -> &str {
                &self.spec.name
            }

            fn description(&self) -> &str {
                &self.spec.description
            }

            fn spec(&self) -> &::strands_agents::types::ToolSpec {
                &self.spec
            }

            async fn invoke(
                &self,
                ctx: ::strands_agents::tools::ToolContext,
            ) -> ::strands_agents::Result<::strands_agents::types::ToolResultBlock> {
                let tool_use_id = ctx.tool_use.tool_use_id.clone();
                let args: #args_name = match ::strands_agents::__private::serde_json::from_value(
                    ctx.tool_use.input.clone(),
                ) {
                    ::core::result::Result::Ok(args) => args,
                    ::core::result::Result::Err(err) => {
                        return ::core::result::Result::Ok(
                            ::strands_agents::tools::create_error_result(&err, &tool_use_id),
                        );
                    }
                };
                #output
                let value = match ::strands_agents::__private::serde_json::to_value(output) {
                    ::core::result::Result::Ok(value) => value,
                    ::core::result::Result::Err(err) => {
                        return ::core::result::Result::Ok(
                            ::strands_agents::tools::create_error_result(&err, &tool_use_id),
                        );
                    }
                };
                ::core::result::Result::Ok(::strands_agents::tools::wrap_in_tool_result(
                    value,
                    &tool_use_id,
                ))
            }
        }
    })
}

fn is_tool_context(ty: &Type) -> bool {
    last_path_segment(ty).is_some_and(|ident| ident == "ToolContext")
}

fn returns_result(output: &ReturnType) -> bool {
    match output {
        ReturnType::Default => false,
        ReturnType::Type(_, ty) => last_path_segment(ty).is_some_and(|ident| ident == "Result"),
    }
}

fn last_path_segment(ty: &Type) -> Option<String> {
    let Type::Path(type_path) = ty else {
        return None;
    };
    type_path
        .path
        .segments
        .last()
        .map(|segment| segment.ident.to_string())
}

/// Leading doc-comment lines, joined; stops at an `# Arguments`-style section.
fn extract_doc_comment(attrs: &[Attribute]) -> String {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let syn::Meta::NameValue(meta) = &attr.meta {
            if let syn::Expr::Lit(expr_lit) = &meta.value {
                if let syn::Lit::Str(lit_str) = &expr_lit.lit {
                    let line = lit_str.value().trim().to_string();
                    let lower = line.to_lowercase();
                    if lower.starts_with("# arg")
                        || lower.starts_with("args:")
                        || lower.starts_with("arguments:")
                    {
                        break;
                    }
                    lines.push(line);
                }
            }
        }
    }
    lines.join(" ").trim().to_string()
}

fn to_pascal_case(input: &str) -> String {
    input
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                Some(first) => first.to_uppercase().chain(chars).collect::<String>(),
                None => String::new(),
            }
        })
        .collect()
}
