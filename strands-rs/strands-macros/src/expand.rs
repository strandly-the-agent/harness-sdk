//! Code generation for `#[tool]` and `#[tools]`.

use proc_macro2::TokenStream;
use quote::{format_ident, quote, quote_spanned};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Attribute, ImplItem, ItemFn, ItemImpl, Visibility, parse_quote};

use crate::args::{ToolsArgs, crate_path};
use crate::signature::{self, Output, Param, Receiver, ToolArgs, ToolFn};

/// `#[tool]` on a free function: the function, a unit struct and its `Tool` impl.
pub(crate) fn tool_fn(args: ToolArgs, mut item: ItemFn) -> syn::Result<TokenStream> {
    let krate = crate_path(args.krate.as_ref());
    let tool = signature::read(&item.attrs, &mut item.sig, args, Receiver::None)?;
    item.attrs.extend(lint_allows(&tool));
    let fn_ident = &tool.ident;
    let mut struct_ident =
        syn::parse_str::<syn::Ident>(&pascal_case(&fn_ident.unraw().to_string())).map_err(|_| {
            syn::Error::new(
                fn_ident.span(),
                format!("#[tool] can't derive a struct name from `{fn_ident}`: start the name with a letter"),
            )
        })?;
    struct_ident.set_span(fn_ident.span());
    let vis = &item.vis;
    let docs = &tool.docs;
    let tool_impl = tool_impl(
        &tool,
        &krate,
        &quote!(#struct_ident),
        &quote!(#fn_ident),
        &TokenStream::new(),
    );
    Ok(quote! {
        #item

        #(#docs)*
        #[derive(Debug, Clone, Copy, Default)]
        #[allow(clippy::exhaustive_structs, reason = "a unit struct that names the tool")]
        #vis struct #struct_ident;

        const _: () = {
            #tool_impl
        };
    })
}

/// `#[tools]` on an impl block: the block without the inner `#[tool]` attributes, a
/// `<method>_tool` constructor per tool method and `tools()`.
pub(crate) fn tools_impl(args: &ToolsArgs, mut item: ItemImpl) -> syn::Result<TokenStream> {
    let krate = crate_path(args.krate.as_ref());
    if let Some((path, _)) = &item.trait_ {
        return Err(syn::Error::new(
            path.span(),
            "#[tools] goes on an inherent impl block",
        ));
    }
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            item.generics.span(),
            "#[tools] impl blocks can't be generic",
        ));
    }
    let self_ty = item.self_ty.clone();
    let list_ident = match &args.list {
        Some(name) => syn::parse_str::<syn::Ident>(name).map_err(|_| {
            syn::Error::new(
                self_ty.span(),
                format!("#[tools(list = \"{name}\")] needs a method name"),
            )
        })?,
        None => format_ident!("tools"),
    };
    let method_idents: Vec<syn::Ident> = item
        .items
        .iter()
        .filter_map(|impl_item| match impl_item {
            ImplItem::Fn(method) => Some(method.sig.ident.clone()),
            _ => None,
        })
        .collect();
    let mut generated = Vec::new();
    let mut constructors: Vec<(syn::Ident, Visibility, Vec<Attribute>)> = Vec::new();
    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let Some(index) = method.attrs.iter().position(is_tool_attr) else {
            continue;
        };
        let attr = method.attrs.remove(index);
        let tool_args = ToolArgs::from_attr(&attr)?;
        let method_krate = tool_args.krate.clone().unwrap_or_else(|| krate.clone());
        let tool = signature::read(&method.attrs, &mut method.sig, tool_args, Receiver::Ref)?;
        method.attrs.extend(lint_allows(&tool));
        let method_ident = &tool.ident;
        if *method_ident == list_ident {
            return Err(syn::Error::new(
                method_ident.span(),
                format!(
                    "a #[tool] method can't be named `{list_ident}`: #[tools] generates a method of that \
                     name. Rename the method, or the generated one with #[tools(list = \"..\")]"
                ),
            ));
        }
        let constructor = format_ident!("{}_tool", method_ident.unraw());
        if let Some(clash) = method_idents.iter().find(|ident| **ident == constructor) {
            return Err(syn::Error::new(
                clash.span(),
                format!("#[tools] generates `{constructor}` for the `{method_ident}` tool: rename this method"),
            ));
        }
        // `#[tools]` expands before the compiler strips cfg'd methods, so the generated items
        // carry the method's `#[cfg]`.
        let cfgs: Vec<Attribute> = method
            .attrs
            .iter()
            .filter(|attr| attr.path().is_ident("cfg"))
            .cloned()
            .collect();
        let vis = &method.vis;
        let docs = &tool.docs;
        let summary = format!("The `{}` tool, bound to this value.", tool.name);
        let bind_this = quote!(let __this = ::std::sync::Arc::clone(&self.0););
        let tool_impl = tool_impl(
            &tool,
            &method_krate,
            &quote!(__StrandsTool),
            &quote!(__this.#method_ident),
            &bind_this,
        );
        generated.push(quote! {
            #(#cfgs)*
            const _: () = {
                struct __StrandsTool(::std::sync::Arc<#self_ty>);

                #tool_impl

                impl #self_ty {
                    #[doc = #summary]
                    #[doc = ""]
                    #(#docs)*
                    #[must_use]
                    #vis fn #constructor(
                        self: &::std::sync::Arc<Self>,
                    ) -> impl #method_krate::__private::Tool + use<> {
                        __StrandsTool(::std::sync::Arc::clone(self))
                    }
                }
            };
        });
        constructors.push((constructor, vis.clone(), cfgs));
    }
    let Some((_, first_vis, _)) = constructors.first() else {
        return Err(syn::Error::new(
            self_ty.span(),
            "#[tools] needs at least one #[tool] method",
        ));
    };
    let vis = constructors
        .iter()
        .map(|(_, vis, _)| vis)
        .find(|vis| matches!(vis, Visibility::Public(_)))
        .unwrap_or(first_vis);
    let pushes = constructors.iter().map(|(ident, _, cfgs)| {
        quote! {
            #(#cfgs)*
            tools.push(#krate::__private::ToolSource::from(self.#ident()));
        }
    });
    Ok(quote! {
        #item

        #(#generated)*

        impl #self_ty {
            /// The tools of the `#[tool]` methods in this impl block, bound to this value, in
            /// declaration order.
            #[must_use]
            #vis fn #list_ident(self: &::std::sync::Arc<Self>) -> ::std::vec::Vec<#krate::__private::ToolSource> {
                #[allow(
                    unused_mut,
                    clippy::vec_init_then_push,
                    reason = "one push per tool, so each can carry the method's #[cfg]"
                )]
                let mut tools = ::std::vec::Vec::new();
                #(#pushes)*
                tools
            }
        }
    })
}

/// The input struct, the cached definition and the `Tool` impl for `target`, which calls
/// `callee` after running `bind_this` in `stream`.
fn tool_impl(
    tool: &ToolFn,
    krate: &syn::Path,
    target: &TokenStream,
    callee: &TokenStream,
    bind_this: &TokenStream,
) -> TokenStream {
    let private = quote!(#krate::__private);
    let serde_crate = format!("{}::__private::serde", quote!(#krate));
    let schemars_crate = format!("{}::__private::schemars", quote!(#krate));
    let name = &tool.name;
    let description = &tool.description;
    let args = tool.params.iter().map(|param| match param {
        Param::Input { ident, .. } => quote!(__input.#ident),
        Param::Context { by_ref: true } => quote!(&__context),
        Param::Context { by_ref: false } => quote!(__context),
    });
    let call = quote!(#callee(#(#args),*));
    let span = match tool.output {
        Output::Result(span) | Output::Value(span) => span,
    };
    // Spanned at the return type, so a non-serializable output is reported there.
    let private_at = private.clone().into_iter().map(|mut token| {
        token.set_span(span);
        token
    });
    let (span, convert) = match tool.output {
        Output::Result(span) => (span, quote_spanned!(span=> #(#private_at)*::result_output)),
        Output::Value(span) => (span, quote_spanned!(span=> #(#private_at)*::value_output)),
    };
    let body = if tool.is_async {
        quote_spanned!(span=> #convert(#call.await))
    } else {
        quote_spanned!(span=> #private::blocking(move || #call).await.and_then(#convert))
    };
    let context = if tool.has_context() {
        quote!(__context)
    } else {
        quote!(_)
    };
    let (input, def, run) = if tool.has_input() {
        let fields = tool.params.iter().filter_map(|param| match param {
            Param::Input { ident, ty, attrs } => Some(quote!(#(#attrs)* #ident: #ty)),
            Param::Context { .. } => None,
        });
        (
            quote! {
                #[derive(#private::serde::Deserialize, #private::schemars::JsonSchema)]
                #[serde(crate = #serde_crate)]
                #[schemars(crate = #schemars_crate)]
                struct __StrandsInput {
                    #(#fields),*
                }
            },
            quote!(__STRANDS_TOOL.typed::<__StrandsInput>(#name, #description)),
            quote! {
                run(context, move |__input: __StrandsInput, #context: #private::ToolContext| async move {
                    #body
                })
            },
        )
    } else {
        (
            TokenStream::new(),
            quote!(__STRANDS_TOOL.untyped(#name, #description)),
            quote! {
                run_without_input(context, move |#context: #private::ToolContext| async move {
                    #body
                })
            },
        )
    };
    quote! {
        #input

        static __STRANDS_TOOL: #private::LazyToolDef = #private::LazyToolDef::new();

        impl #private::Tool for #target {
            fn name(&self) -> &str {
                #name
            }

            fn description(&self) -> &str {
                #description
            }

            fn tool_spec(&self) -> #private::ToolSpec {
                #def.spec()
            }

            fn stream(&self, context: #private::ToolContext) -> #private::ToolStream {
                #bind_this
                #def.#run
            }
        }
    }
}

/// Lints the macro's own contract triggers on the re-emitted function.
fn lint_allows(tool: &ToolFn) -> Vec<Attribute> {
    let mut allows = Vec::new();
    if !tool.params.is_empty() {
        allows.push(parse_quote!(#[allow(
            clippy::needless_pass_by_value,
            reason = "tool parameters are owned: they are deserialized from the tool input"
        )]));
    }
    if tool.is_async {
        allows.push(parse_quote!(#[allow(
            clippy::unused_async,
            reason = "an async tool body runs on the tool task, a sync one on a blocking thread"
        )]));
    }
    allows
}

fn is_tool_attr(attr: &Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|last| last.ident == "tool")
}

/// `get_weather` becomes `GetWeather`.
fn pascal_case(name: &str) -> String {
    name.split('_')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            chars
                .next()
                .map(|first| first.to_uppercase().chain(chars).collect::<String>())
                .unwrap_or_default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_code_uses_only_the_given_crate_path() {
        let args = ToolArgs {
            name: None,
            krate: Some(parse_quote!(::my_strands)),
        };
        let item: ItemFn = parse_quote! {
            /// Echo the text.
            async fn echo(
                /// The text.
                text: String,
            ) -> String {
                text
            }
        };
        let code = tool_fn(args, item).unwrap().to_string();
        assert!(code.contains("my_strands"));
        assert!(!code.replace("my_strands", "").contains("strands ::"), "{code}");

        let tools: ItemImpl = parse_quote! {
            impl Store {
                /// Count.
                #[tool]
                fn count(&self) -> u32 {
                    0
                }
            }
        };
        let args = ToolsArgs {
            krate: Some(parse_quote!(::my_strands)),
            list: None,
        };
        let code = tools_impl(&args, tools).unwrap().to_string();
        assert!(!code.replace("my_strands", "").contains("strands ::"), "{code}");
    }
}
