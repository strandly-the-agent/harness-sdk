//! `#[tools]` on impl blocks.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{Ident, ImplItem, ItemImpl, Visibility};

use crate::tool::{Receiver, ToolArgs, ToolFn, owned_params_allow};

pub(crate) fn expand(attr: &TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new(attr.span(), "#[tools] takes no arguments"));
    }
    let mut item: ItemImpl = syn::parse2(item)?;
    if let Some((path, _)) = &item.trait_ {
        return Err(syn::Error::new(
            path.span(),
            "#[tools] goes on an inherent impl block, not a trait impl",
        ));
    }
    if !item.generics.params.is_empty() || item.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            item.generics.span(),
            "#[tools] impl blocks can't be generic",
        ));
    }
    let self_ty = &item.self_ty;
    let mut constructors = Vec::new();
    let mut tools_vis: Option<Visibility> = None;
    for impl_item in &mut item.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let Some(position) = method.attrs.iter().position(is_tool_attr) else {
            continue;
        };
        let attr = method.attrs.remove(position);
        method.attrs.push(owned_params_allow());
        let tool = ToolFn::parse(
            ToolArgs::from_attribute(&attr)?,
            &method.attrs,
            &mut method.sig,
            &Receiver::Method,
        )?;
        let vis = &method.vis;
        tools_vis.get_or_insert_with(|| vis.clone());
        let method_ident = method.sig.ident.unraw();
        let constructor = format_ident!("{}_tool", method_ident);
        let this = Ident::new("this", Span::mixed_site());
        let tool_impl = tool.tool_impl(&quote! { MethodTool }, Some(&this));
        let doc = format!("The `{}` tool, which calls [`Self::{method_ident}`].", tool.name);
        constructors.push((
            constructor.clone(),
            quote! {
                #[doc = #doc]
                #[must_use]
                #vis fn #constructor(self: &::std::sync::Arc<Self>) -> ::std::sync::Arc<dyn ::strands::__private::Tool> {
                    struct MethodTool(::std::sync::Arc<#self_ty>);

                    #tool_impl

                    ::std::sync::Arc::new(MethodTool(::std::sync::Arc::clone(self)))
                }
            },
        ));
    }
    let Some(tools_vis) = tools_vis else {
        return Err(syn::Error::new(
            item.self_ty.span(),
            "#[tools] found no #[tool] methods",
        ));
    };
    let names = constructors.iter().map(|(constructor, _)| constructor);
    let bodies = constructors.iter().map(|(_, body)| body);
    Ok(quote! {
        #item

        impl #self_ty {
            #(#bodies)*

            /// Every tool of this type, in declaration order, for `.tools(..)`.
            #[must_use]
            #tools_vis fn tools(self: &::std::sync::Arc<Self>) -> ::std::vec::Vec<::strands::__private::ToolSource> {
                ::std::vec![#(::strands::__private::ToolSource::from(self.#names())),*]
            }
        }
    })
}

fn is_tool_attr(attr: &syn::Attribute) -> bool {
    attr.path()
        .segments
        .last()
        .is_some_and(|segment| segment.ident == "tool")
}
