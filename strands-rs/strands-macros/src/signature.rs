//! Reads a `#[tool]` function signature into a [`ToolFn`], stripping the parameter attributes
//! rustc would reject after expansion.

use darling::FromMeta;
use proc_macro2::Span;
use syn::ext::IdentExt;
use syn::spanned::Spanned;
use syn::{
    Attribute, Expr, ExprLit, FnArg, Ident, Lit, Meta, Pat, ReceiverKind, ReturnType, Safety, Signature, Type,
};

use crate::args::ParamArgs;
pub(crate) use crate::args::ToolArgs;

/// Where the function sits, which decides the receiver it may take.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Receiver {
    /// A free function: no receiver.
    None,
    /// A method in a `#[tools]` impl block: `&self`.
    Ref,
}

/// A parameter of the tool function.
pub(crate) enum Param {
    /// A property of the tool input.
    Input {
        ident: Ident,
        ty: Box<Type>,
        attrs: Vec<Attribute>,
    },
    /// The `#[tool(context)]` parameter.
    Context { by_ref: bool },
}

/// What the function returns.
pub(crate) enum Output {
    /// `Result<T, E>`: `T: Serialize`, `E: Into<ToolError>`.
    Result(Span),
    /// A plain serializable value.
    Value(Span),
}

/// A function the macros turn into a tool.
pub(crate) struct ToolFn {
    pub(crate) ident: Ident,
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) docs: Vec<Attribute>,
    pub(crate) is_async: bool,
    pub(crate) params: Vec<Param>,
    pub(crate) output: Output,
}

impl ToolFn {
    /// Whether any parameter is an input property.
    pub(crate) fn has_input(&self) -> bool {
        self.params
            .iter()
            .any(|param| matches!(param, Param::Input { .. }))
    }

    /// Whether a parameter takes the tool context.
    pub(crate) fn has_context(&self) -> bool {
        self.params
            .iter()
            .any(|param| matches!(param, Param::Context { .. }))
    }
}

/// Reads `sig` and its doc attributes, and strips the parameter attributes from `sig`.
pub(crate) fn read(
    attrs: &[Attribute],
    sig: &mut Signature,
    args: ToolArgs,
    receiver: Receiver,
) -> syn::Result<ToolFn> {
    check_shape(sig)?;
    let ident = sig.ident.clone();
    let name = args.name.unwrap_or_else(|| ident.unraw().to_string());
    if !is_valid_tool_name(&name) {
        return Err(syn::Error::new(
            ident.span(),
            format!("invalid tool name `{name}`: use 1 to 64 characters of [a-zA-Z0-9_-]"),
        ));
    }
    let docs: Vec<Attribute> = attrs
        .iter()
        .filter(|attr| attr.path().is_ident("doc"))
        .cloned()
        .collect();
    let description = doc_text(&docs);
    if description.is_empty() {
        return Err(syn::Error::new(
            ident.span(),
            "#[tool] needs a doc comment: it is the tool description the model sees, and tool \
             registration rejects an empty description",
        ));
    }
    let mut params = Vec::new();
    for arg in &mut sig.inputs {
        if let Some(param) = read_param(arg, receiver)? {
            params.push(param);
        }
    }
    if params
        .iter()
        .filter(|param| matches!(param, Param::Context { .. }))
        .count()
        > 1
    {
        return Err(syn::Error::new(
            sig.inputs.span(),
            "only one parameter can be #[tool(context)]",
        ));
    }
    if receiver == Receiver::Ref && !matches!(sig.inputs.first(), Some(FnArg::Receiver(_))) {
        return Err(syn::Error::new(
            ident.span(),
            "#[tool] methods in #[tools] take `&self`",
        ));
    }
    Ok(ToolFn {
        ident,
        name,
        description,
        docs,
        is_async: sig.asyncness.is_some(),
        params,
        output: output(&sig.output),
    })
}

fn check_shape(sig: &Signature) -> syn::Result<()> {
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            sig.generics.span(),
            "#[tool] functions can't be generic: the input schema is derived from concrete types",
        ));
    }
    if let Safety::Unsafe(token) = &sig.safety {
        return Err(syn::Error::new(token.span, "#[tool] functions can't be unsafe"));
    }
    Ok(())
}

fn read_param(arg: &mut FnArg, receiver: Receiver) -> syn::Result<Option<Param>> {
    let typed = match arg {
        FnArg::Receiver(found) => return check_receiver(found, receiver).map(|()| None),
        FnArg::Typed(typed) => typed,
    };
    let mut is_context = false;
    let mut field_attrs = Vec::new();
    for attr in std::mem::take(&mut typed.attrs) {
        if attr.path().is_ident("tool") {
            is_context |= context_flag(&attr)?;
        } else if ["doc", "serde", "schemars"]
            .iter()
            .any(|name| attr.path().is_ident(name))
        {
            field_attrs.push(attr);
        } else {
            typed.attrs.push(attr);
        }
    }
    if is_context {
        if let Type::Reference(reference) = &*typed.ty
            && reference.mutability.is_some()
        {
            return Err(syn::Error::new(
                typed.ty.span(),
                "the `#[tool(context)]` parameter is `&ToolContext` or `ToolContext`",
            ));
        }
        return Ok(Some(Param::Context {
            by_ref: matches!(*typed.ty, Type::Reference(_)),
        }));
    }
    let ident = param_ident(&typed.pat)?;
    check_input_type(&typed.ty)?;
    Ok(Some(Param::Input {
        ident,
        ty: typed.ty.clone(),
        attrs: field_attrs,
    }))
}

fn check_receiver(found: &syn::Receiver, receiver: Receiver) -> syn::Result<()> {
    match (receiver, &found.kind) {
        (Receiver::None, _) => Err(syn::Error::new(
            found.span(),
            "#[tool] on a method needs #[tools] on its impl block",
        )),
        (Receiver::Ref, ReceiverKind::Reference(_, _, None)) => Ok(()),
        (Receiver::Ref, _) => Err(syn::Error::new(
            found.span(),
            "#[tool] methods in #[tools] take `&self`",
        )),
    }
}

fn context_flag(attr: &Attribute) -> syn::Result<bool> {
    let args = match &attr.meta {
        Meta::List(_) => ParamArgs::from_meta(&attr.meta)?,
        _ => return Err(syn::Error::new(attr.span(), "expected #[tool(context)]")),
    };
    Ok(args.context.is_present())
}

fn param_ident(pat: &Pat) -> syn::Result<Ident> {
    match pat {
        Pat::Ident(pat) if pat.by_ref.is_none() && pat.subpat.is_none() => Ok(pat.ident.clone()),
        other => Err(syn::Error::new(
            other.span(),
            "#[tool] parameters need a plain name: it is the input property name",
        )),
    }
}

fn check_input_type(ty: &Type) -> syn::Result<()> {
    let names_context = |ty: &Type| matches!(ty, Type::Path(path) if path.path.segments.last().is_some_and(|last| last.ident == "ToolContext"));
    match ty {
        Type::Reference(reference) if names_context(&reference.elem) => Err(context_hint(&reference.elem)),
        _ if names_context(ty) => Err(context_hint(ty)),
        Type::Reference(_) => Err(syn::Error::new(
            ty.span(),
            "#[tool] parameters must be owned types: they are deserialized from the tool input",
        )),
        Type::ImplTrait(_) => Err(syn::Error::new(
            ty.span(),
            "#[tool] functions can't be generic: the input schema is derived from concrete types",
        )),
        _ => Ok(()),
    }
}

fn context_hint(ty: &Type) -> syn::Error {
    syn::Error::new(
        ty.span(),
        "mark the `ToolContext` parameter with #[tool(context)]",
    )
}

fn output(output: &ReturnType) -> Output {
    let ReturnType::Type(_, ty) = output else {
        return Output::Value(Span::call_site());
    };
    let is_result = matches!(&**ty, Type::Path(path)
        if path.path.segments.last().is_some_and(|last| last.ident == "Result"));
    if is_result {
        Output::Result(ty.span())
    } else {
        Output::Value(ty.span())
    }
}

/// The text of the literal doc attributes, one line each, without rustdoc's leading space, up to
/// the first rustdoc section heading (`# Errors`, `# Examples`, ...).
fn doc_text(docs: &[Attribute]) -> String {
    let lines: Vec<String> = docs
        .iter()
        .filter_map(|attr| match &attr.meta {
            Meta::NameValue(pair) => match &pair.value {
                Expr::Lit(ExprLit {
                    lit: Lit::Str(text), ..
                }) => Some(text.value()),
                _ => None,
            },
            _ => None,
        })
        .map(|line| line.strip_prefix(' ').unwrap_or(&line).to_owned())
        .take_while(|line| !line.starts_with("# "))
        .collect();
    lines.join("\n").trim().to_owned()
}

/// TS `isValidToolName`: 1 to 64 characters of `[a-zA-Z0-9_-]`.
fn is_valid_tool_name(name: &str) -> bool {
    (1..=64).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
}
