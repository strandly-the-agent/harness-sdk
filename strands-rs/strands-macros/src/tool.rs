//! `#[tool]` on functions, and the code generation `#[tools]` shares.

use darling::FromMeta;
use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, FnArg, Ident, ItemFn, Lit, Meta, Pat, ReceiverKind, ReturnType, Signature, Type,
};

const MAX_TOOL_NAME_LENGTH: usize = 64;

/// Tool parameters are deserialized into owned values, so a body that only reads them is fine.
pub(crate) fn owned_params_allow() -> Attribute {
    syn::parse_quote! {
        #[allow(clippy::needless_pass_by_value, reason = "tool parameters are deserialized into owned values")]
    }
}

pub(crate) use args::{ParamArgs, ToolArgs};

mod args {
    #![expect(
        clippy::unreachable,
        reason = "darling's FromMeta derive expands to unreachable!"
    )]

    use darling::FromMeta;
    use darling::ast::NestedMeta;
    use proc_macro2::TokenStream;
    use syn::{Attribute, Meta};

    /// `#[tool(..)]` arguments on a function or method.
    #[derive(Debug, Default, FromMeta)]
    pub(crate) struct ToolArgs {
        pub(crate) name: Option<String>,
    }

    impl ToolArgs {
        pub(crate) fn parse(tokens: TokenStream) -> syn::Result<Self> {
            let items = NestedMeta::parse_meta_list(tokens)?;
            Ok(Self::from_list(&items)?)
        }

        pub(crate) fn from_attribute(attr: &Attribute) -> syn::Result<Self> {
            match &attr.meta {
                Meta::Path(_) => Ok(Self::default()),
                meta => Ok(Self::from_meta(meta)?),
            }
        }
    }

    /// `#[tool(..)]` arguments on a parameter.
    #[derive(Debug, Default, FromMeta)]
    pub(crate) struct ParamArgs {
        #[darling(default)]
        pub(crate) context: bool,
    }
}

/// Where the tool function lives.
pub(crate) enum Receiver {
    /// A free function.
    None,
    /// A `&self` method of the `#[tools]` impl type.
    Method,
}

enum Arg {
    Context { by_ref: bool },
    Input(usize),
}

struct Input {
    ident: Ident,
    ty: Type,
    docs: Vec<Attribute>,
}

/// A parsed tool function: what the generated `Tool` impl needs.
pub(crate) struct ToolFn {
    fn_ident: Ident,
    pub(crate) name: String,
    description: String,
    is_async: bool,
    returns_result: bool,
    inputs: Vec<Input>,
    args: Vec<Arg>,
}

impl ToolFn {
    /// Parses `sig` and strips the `#[tool]` and doc attributes from its parameters, which rustc
    /// rejects there.
    pub(crate) fn parse(
        args: ToolArgs,
        attrs: &[Attribute],
        sig: &mut Signature,
        receiver: &Receiver,
    ) -> syn::Result<Self> {
        if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
            return Err(syn::Error::new(
                sig.generics.span(),
                "#[tool] functions can't be generic",
            ));
        }
        let fn_ident = sig.ident.clone();
        let name = args.name.unwrap_or_else(|| fn_ident.unraw().to_string());
        if !is_valid_tool_name(&name) {
            return Err(syn::Error::new(
                fn_ident.span(),
                format!("invalid tool name `{name}`: use 1 to 64 ASCII letters, digits, `_` or `-`"),
            ));
        }
        let description = doc_text(attrs);
        if description.is_empty() {
            return Err(syn::Error::new(
                fn_ident.span(),
                "#[tool] functions need a doc comment: it is the tool description the model sees",
            ));
        }
        let (inputs, args) = parse_params(sig, receiver)?;
        Ok(Self {
            fn_ident,
            name,
            description,
            is_async: sig.asyncness.is_some(),
            returns_result: returns_result(&sig.output),
            inputs,
            args,
        })
    }

    /// The input struct, its schema accessor and the `Tool` impl for `tool_ty`. With `this`, the
    /// function is a method called on the `Arc<Self>` in `tool_ty`'s field `0`.
    pub(crate) fn tool_impl(&self, tool_ty: &TokenStream, this: Option<&Ident>) -> TokenStream {
        let name = &self.name;
        let description = &self.description;
        let fields = self.inputs.iter().map(|input| {
            let Input { ident, ty, docs } = input;
            quote! { #(#docs)* #ident: #ty }
        });
        let context = Ident::new("context", Span::mixed_site());
        let output = Ident::new("output", Span::mixed_site());
        let tool_use_id = Ident::new("tool_use_id", Span::mixed_site());
        let bindings: Vec<Ident> = (0..self.inputs.len())
            .map(|index| format_ident!("arg{}", index, span = Span::mixed_site()))
            .collect();
        let field_idents = self.inputs.iter().map(|input| &input.ident);
        let call = self.call(&context, &bindings, this);
        let clone_this = this.map(|this| quote! { let #this = ::std::sync::Arc::clone(&self.0); });
        quote! {
            #[derive(::strands::__private::serde::Deserialize, ::strands::__private::schemars::JsonSchema)]
            #[serde(crate = "::strands::__private::serde")]
            #[schemars(crate = "::strands::__private::schemars")]
            struct ToolInput {
                #(#fields,)*
            }

            fn tool_input_schema() -> &'static ::strands::__private::TypedInputSchema {
                static SCHEMA: ::std::sync::OnceLock<::strands::__private::TypedInputSchema> =
                    ::std::sync::OnceLock::new();
                SCHEMA.get_or_init(::strands::__private::TypedInputSchema::new::<ToolInput>)
            }

            impl ::strands::__private::Tool for #tool_ty {
                fn name(&self) -> &str {
                    #name
                }

                fn description(&self) -> &str {
                    #description
                }

                fn tool_spec(&self) -> ::strands::__private::ToolSpec {
                    ::strands::__private::ToolSpec::new(#name, #description)
                        .with_input_schema(::std::clone::Clone::clone(tool_input_schema().input_schema()))
                }

                fn stream(&self, #context: ::strands::__private::ToolContext) -> ::strands::__private::ToolStream {
                    #clone_this
                    ::strands::__private::ToolStream::from_future(async move {
                        let #tool_use_id = ::std::clone::Clone::clone(&#context.tool_use().tool_use_id);
                        let #output = async {
                            let ToolInput { #(#field_idents: #bindings),* } =
                                tool_input_schema().parse::<ToolInput>(&#context.tool_use().input)?;
                            #call
                        }
                        .await;
                        ::strands::__private::finish(#tool_use_id, #output)
                    })
                }
            }
        }
    }

    fn call(&self, context: &Ident, bindings: &[Ident], this: Option<&Ident>) -> TokenStream {
        let fn_ident = &self.fn_ident;
        let blocking_context = Ident::new("blocking_context", Span::mixed_site());
        let ctx = if self.is_async { context } else { &blocking_context };
        let args = self.args.iter().map(|arg| match arg {
            Arg::Context { by_ref: true } => quote! { &#ctx },
            Arg::Context { by_ref: false } => quote! { ::std::clone::Clone::clone(&#ctx) },
            Arg::Input(index) => {
                let binding = &bindings[*index];
                quote! { #binding }
            }
        });
        let invoke = match this {
            Some(this) => quote! { #this.#fn_ident(#(#args),*) },
            None => quote! { #fn_ident(#(#args),*) },
        };
        let value = if self.is_async {
            quote! { #invoke.await }
        } else {
            let take_context = self
                .args
                .iter()
                .any(|arg| matches!(arg, Arg::Context { .. }))
                .then(|| quote! { let #blocking_context = ::std::clone::Clone::clone(&#context); });
            quote! {{
                #take_context
                ::strands::__private::run_blocking(move || #invoke).await?
            }}
        };
        if self.returns_result {
            quote! { ::strands::__private::serialize_output(#value) }
        } else {
            quote! { ::strands::__private::serialize_output(::std::result::Result::<_, ::strands::__private::ToolError>::Ok(#value)) }
        }
    }
}

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args = ToolArgs::parse(attr)?;
    let mut item: ItemFn = syn::parse2(item)?;
    let tool = ToolFn::parse(args, &item.attrs, &mut item.sig, &Receiver::None)?;
    let vis = item.vis.clone();
    let docs: Vec<Attribute> = item
        .attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .cloned()
        .collect();
    let struct_ident = Ident::new(
        &pascal_case(&item.sig.ident.unraw().to_string()),
        item.sig.ident.span(),
    );
    let tool_impl = tool.tool_impl(&quote! { #struct_ident }, None);
    item.attrs.push(owned_params_allow());
    Ok(quote! {
        #item

        #(#docs)*
        #[derive(Debug, Clone, Copy, Default)]
        #vis struct #struct_ident;

        const _: () = {
            #tool_impl
        };
    })
}

fn parse_params(sig: &mut Signature, receiver: &Receiver) -> syn::Result<(Vec<Input>, Vec<Arg>)> {
    let mut inputs = Vec::new();
    let mut args = Vec::new();
    for param in &mut sig.inputs {
        let param = match param {
            FnArg::Receiver(found) => {
                check_receiver(found, receiver)?;
                continue;
            }
            FnArg::Typed(param) => param,
        };
        let param_args = take_param_args(&mut param.attrs)?;
        let docs: Vec<Attribute> = take_docs(&mut param.attrs);
        if param_args.context {
            if args.iter().any(|arg| matches!(arg, Arg::Context { .. })) {
                return Err(syn::Error::new(
                    param.span(),
                    "only one parameter can be #[tool(context)]",
                ));
            }
            args.push(Arg::Context {
                by_ref: matches!(*param.ty, Type::Reference(_)),
            });
            continue;
        }
        let Pat::Ident(pat) = &*param.pat else {
            return Err(syn::Error::new(
                param.pat.span(),
                "#[tool] parameters must be plain names",
            ));
        };
        if pat.by_ref.is_some() || pat.subpat.is_some() {
            return Err(syn::Error::new(
                param.pat.span(),
                "#[tool] parameters must be plain names",
            ));
        }
        if let Type::Reference(reference) = &*param.ty {
            return Err(syn::Error::new(
                reference.span(),
                "#[tool] parameters must be owned types (`String` rather than `&str`); only the #[tool(context)] parameter can be a reference",
            ));
        }
        args.push(Arg::Input(inputs.len()));
        inputs.push(Input {
            ident: pat.ident.clone(),
            ty: (*param.ty).clone(),
            docs,
        });
    }
    if matches!(receiver, Receiver::Method) && !matches!(sig.inputs.first(), Some(FnArg::Receiver(_))) {
        return Err(syn::Error::new(
            sig.ident.span(),
            "#[tool] methods must take `&self`",
        ));
    }
    Ok((inputs, args))
}

fn check_receiver(found: &syn::Receiver, receiver: &Receiver) -> syn::Result<()> {
    match receiver {
        Receiver::None => Err(syn::Error::new(
            found.span(),
            "#[tool] on a method needs #[tools] on its impl block",
        )),
        Receiver::Method => match &found.kind {
            ReceiverKind::Reference(_, _, None) => Ok(()),
            _ => Err(syn::Error::new(found.span(), "#[tool] methods must take `&self`")),
        },
    }
}

fn take_param_args(attrs: &mut Vec<Attribute>) -> syn::Result<ParamArgs> {
    let mut args = ParamArgs::default();
    let mut error = None;
    attrs.retain(|attr| {
        if !attr.path().is_ident("tool") {
            return true;
        }
        match ParamArgs::from_meta(&attr.meta) {
            Ok(parsed) => args.context |= parsed.context,
            Err(parse_error) => error = Some(syn::Error::from(parse_error)),
        }
        false
    });
    error.map_or(Ok(args), Err)
}

fn take_docs(attrs: &mut Vec<Attribute>) -> Vec<Attribute> {
    let (docs, rest): (Vec<Attribute>, Vec<Attribute>) =
        attrs.drain(..).partition(|attr| attr.path().is_ident("doc"));
    *attrs = rest;
    docs
}

/// The doc comment as text: one leading space per line removed, lines joined, ends trimmed.
pub(crate) fn doc_text(attrs: &[Attribute]) -> String {
    let lines: Vec<String> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(meta) => match &meta.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(text), ..
                }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
        .flat_map(|text| {
            text.split('\n')
                .map(|line| line.strip_prefix(' ').unwrap_or(line).to_owned())
                .collect::<Vec<_>>()
        })
        .collect();
    lines.join("\n").trim().to_owned()
}

fn returns_result(output: &ReturnType) -> bool {
    match output {
        ReturnType::Type(_, ty) => match &**ty {
            Type::Path(path) => path
                .path
                .segments
                .last()
                .is_some_and(|segment| segment.ident == "Result"),
            _ => false,
        },
        ReturnType::Default => false,
    }
}

fn is_valid_tool_name(name: &str) -> bool {
    (1..=MAX_TOOL_NAME_LENGTH).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}

pub(crate) fn pascal_case(name: &str) -> String {
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
