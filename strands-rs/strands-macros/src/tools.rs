//! `#[tools]` on impl blocks.

use proc_macro2::TokenStream;
use quote::{format_ident, quote};
use syn::{ImplItem, ItemFn, ItemImpl, Type};

use crate::tool::{ToolFn, parse_args, pascal_case};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    if !attr.is_empty() {
        return Err(syn::Error::new_spanned(attr, "#[tools] takes no arguments"));
    }
    let mut block: ItemImpl = syn::parse2(item)?;
    if block.trait_.is_some() || !block.generics.params.is_empty() {
        return Err(syn::Error::new_spanned(
            &block.self_ty,
            "#[tools] goes on a non-generic inherent impl block",
        ));
    }
    let self_ty: Type = block.self_ty.as_ref().clone();
    let type_name = match &self_ty {
        Type::Path(path) => path.path.segments.last().map(|segment| segment.ident.to_string()),
        _ => None,
    }
    .unwrap_or_else(|| "Tools".to_owned());
    let mut generated = Vec::new();
    let mut constructors = Vec::new();
    let mut tool_idents = Vec::new();
    for impl_item in &mut block.items {
        let ImplItem::Fn(method) = impl_item else {
            continue;
        };
        let Some(position) = method.attrs.iter().position(|attr| attr.path().is_ident("tool")) else {
            continue;
        };
        let attr = method.attrs.remove(position);
        let args = match &attr.meta {
            syn::Meta::Path(_) => Default::default(),
            meta => parse_args(meta.require_list()?.tokens.clone())?,
        };
        let mut as_fn = ItemFn {
            attrs: method.attrs.clone(),
            vis: method.vis.clone(),
            modifiers: method.modifiers.clone(),
            sig: method.sig.clone(),
            block: Box::new(method.block.clone()),
        };
        let tool = ToolFn::parse(&mut as_fn, args)?;
        method.sig = as_fn.sig;
        if !tool.has_receiver {
            return Err(syn::Error::new_spanned(
                &method.sig,
                "#[tool] methods take `&self`",
            ));
        }
        let method_ident = &tool.ident;
        let struct_ident = format_ident!(
            "__StrandsTool{}{}",
            type_name,
            pascal_case(&method_ident.to_string())
        );
        let constructor = format_ident!("{}_tool", method_ident);
        let tool_impl = tool.impl_tool(&quote!(#struct_ident), &quote!(#method_ident));
        let doc = format!("The `{}` tool, calling [`Self::{}`].", tool.name, method_ident);
        generated.push(quote! {
            struct #struct_ident(::std::sync::Arc<#self_ty>);

            const _: () = {
                #tool_impl
            };
        });
        constructors.push(quote! {
            #[doc = #doc]
            #[must_use]
            pub fn #constructor(self: &::std::sync::Arc<Self>) -> impl ::strands::Tool {
                #struct_ident(::std::sync::Arc::clone(self))
            }
        });
        tool_idents.push(constructor);
    }
    Ok(quote! {
        #block

        #(#generated)*

        impl #self_ty {
            #(#constructors)*

            /// Every `#[tool]` method of this type, as tools.
            #[must_use]
            pub fn tools(self: &::std::sync::Arc<Self>) -> ::std::vec::Vec<::strands::ToolSource> {
                ::std::vec![#(::strands::ToolSource::from(self.#tool_idents())),*]
            }
        }
    })
}
