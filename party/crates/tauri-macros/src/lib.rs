//! `#[command]` and `generate_handler!` for the headless server build.
//!
//! `#[command]` leaves the function alone and adds a sibling `__cmd_<name>` that reads the arguments out of an
//! `Invocation` and serialises the result. `generate_handler!` turns a list of command paths into a dispatch closure.

use proc_macro::TokenStream;
use proc_macro2::{Group, TokenTree};
use quote::{format_ident, quote};
use syn::parse::{Parse, ParseStream, Parser};
use syn::punctuated::Punctuated;
use syn::{Attribute, FnArg, ItemFn, Pat, Path, ReturnType, Token, Type};

/// `snake_case` to the `camelCase` key the UI sends (what Tauri's own macro does).
fn camel(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut upper = false;
    for c in name.chars() {
        if c == '_' {
            upper = !out.is_empty();
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

fn returns_result(ret: &ReturnType) -> bool {
    let ReturnType::Type(_, ty) = ret else {
        return false;
    };
    let Type::Path(path) = ty.as_ref() else {
        return false;
    };
    path.path
        .segments
        .last()
        .is_some_and(|s| s.ident.to_string().ends_with("Result"))
}

/// Every named lifetime becomes `'_`, so a parameter's type can be written in a `let`.
fn anonymous_lifetimes(tokens: proc_macro2::TokenStream) -> proc_macro2::TokenStream {
    let mut out = proc_macro2::TokenStream::new();
    let mut after_tick = false;
    for tree in tokens {
        match tree {
            TokenTree::Punct(p) if p.as_char() == '\'' => {
                after_tick = true;
                out.extend(quote!('_));
            }
            TokenTree::Ident(_) if after_tick => after_tick = false,
            TokenTree::Group(g) => {
                let inner = anonymous_lifetimes(g.stream());
                out.extend([TokenTree::Group(Group::new(g.delimiter(), inner))]);
            }
            other => out.extend([other]),
        }
    }
    out
}

/// Marks a backend command.
#[proc_macro_attribute]
pub fn command(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let func = syn::parse_macro_input!(item as ItemFn);
    let name = &func.sig.ident;
    let wrapper = format_ident!("__cmd_{name}");
    let vis = &func.vis;
    let mut reads = Vec::new();
    let mut idents = Vec::new();
    for (i, arg) in func.sig.inputs.iter().enumerate() {
        let FnArg::Typed(arg) = arg else {
            return syn::Error::new_spanned(arg, "commands cannot take self")
                .to_compile_error()
                .into();
        };
        let key = match arg.pat.as_ref() {
            Pat::Ident(p) => camel(&p.ident.to_string()),
            _ => format!("arg{i}"),
        };
        let ident = format_ident!("__arg{i}");
        let ty = &arg.ty;
        let ty = anonymous_lifetimes(quote!(#ty));
        reads.push(quote! {
            let #ident: #ty = ::tauri::CommandArg::from_invocation(#key, __invocation)?;
        });
        idents.push(ident);
    }
    let call = if func.sig.asyncness.is_some() {
        quote! { ::tauri::block_on(#name(#(#idents),*)) }
    } else {
        quote! { #name(#(#idents),*) }
    };
    let finish = if returns_result(&func.sig.output) {
        quote! { ::tauri::finish_result(#call) }
    } else {
        quote! { ::tauri::finish_value(#call) }
    };
    quote! {
        #func

        #[doc(hidden)]
        #[allow(clippy::all, clippy::pedantic, missing_docs)]
        #vis fn #wrapper(__invocation: &::tauri::Invocation)
            -> ::std::result::Result<::tauri::serde_json::Value, ::tauri::serde_json::Value>
        {
            #(#reads)*
            #finish
        }
    }
    .into()
}

struct Entry {
    attrs: Vec<Attribute>,
    path: Path,
}

impl Parse for Entry {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        Ok(Self {
            attrs: input.call(Attribute::parse_outer)?,
            path: input.parse()?,
        })
    }
}

/// A dispatcher over the listed commands: `|name, invocation| -> Option<Result<Value, Value>>`.
#[proc_macro]
pub fn generate_handler(input: TokenStream) -> TokenStream {
    let parser = Punctuated::<Entry, Token![,]>::parse_terminated;
    let entries = match parser.parse(input) {
        Ok(p) => p,
        Err(e) => return e.to_compile_error().into(),
    };
    let mut arms = Vec::new();
    for Entry { attrs, path } in entries {
        let mut wrapper = path.clone();
        let last = wrapper.segments.last_mut().expect("a command path");
        let name = last.ident.to_string();
        last.ident = format_ident!("__cmd_{}", last.ident);
        arms.push(quote! {
            #(#attrs)*
            #name => ::std::option::Option::Some(#wrapper(__invocation)),
        });
    }
    quote! {
        |__name: &str, __invocation: &::tauri::Invocation|
            -> ::std::option::Option<::std::result::Result<::tauri::serde_json::Value, ::tauri::serde_json::Value>>
        {
            match __name {
                #(#arms)*
                _ => ::std::option::Option::None,
            }
        }
    }
    .into()
}
