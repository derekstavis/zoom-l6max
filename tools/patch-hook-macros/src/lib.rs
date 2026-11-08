use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{FnArg, Ident, ItemFn, LitInt, Pat, ReturnType, Token, parse::Parse, parse_macro_input};

struct CallHookArgs {
    function: Ident,
    at: LitInt,
}

struct SiteId(Ident);

impl Parse for SiteId {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let site = input.parse()?;
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after hook site"));
        }
        Ok(Self(site))
    }
}

struct OffsetHookArgs {
    function: Ident,
    at: LitInt,
    displaced_len: LitInt,
    guard_sha256: syn::LitStr,
}

impl Parse for OffsetHookArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let function = input.parse()?;
        input.parse::<Token![,]>()?;
        let key: Ident = input.parse()?;
        if key != "at" {
            return Err(syn::Error::new_spanned(key, "expected `at = OFFSET`"));
        }
        input.parse::<Token![=]>()?;
        let at = input.parse()?;
        input.parse::<Token![,]>()?;
        let key: Ident = input.parse()?;
        if key != "displaced_len" {
            return Err(syn::Error::new_spanned(key, "expected `displaced_len = N`"));
        }
        input.parse::<Token![=]>()?;
        let displaced_len = input.parse()?;
        input.parse::<Token![,]>()?;
        let key: Ident = input.parse()?;
        if key != "guard_sha256" {
            return Err(syn::Error::new_spanned(
                key,
                "expected `guard_sha256 = \"...\"`",
            ));
        }
        input.parse::<Token![=]>()?;
        let guard_sha256 = input.parse()?;
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after offset guard"));
        }
        Ok(Self {
            function,
            at,
            displaced_len,
            guard_sha256,
        })
    }
}

impl Parse for CallHookArgs {
    fn parse(input: syn::parse::ParseStream<'_>) -> syn::Result<Self> {
        let function = input.parse()?;
        input.parse::<Token![,]>()?;
        let key: Ident = input.parse()?;
        if key != "at" {
            return Err(syn::Error::new_spanned(key, "expected `at = ADDRESS`"));
        }
        input.parse::<Token![=]>()?;
        let at = input.parse()?;
        if !input.is_empty() {
            return Err(input.error("unexpected tokens after call-site address"));
        }
        Ok(Self { function, at })
    }
}

#[proc_macro_attribute]
pub fn function_call_pre_hook(args: TokenStream, input: TokenStream) -> TokenStream {
    call_hook(args, input, "pre")
}

#[proc_macro_attribute]
pub fn function_call_post_hook(args: TokenStream, input: TokenStream) -> TokenStream {
    call_hook(args, input, "post")
}

fn call_hook(args: TokenStream, input: TokenStream, phase: &str) -> TokenStream {
    let args = parse_macro_input!(args as CallHookArgs);
    let mut function = parse_macro_input!(input as ItemFn);
    let handler = function.sig.ident.clone();

    if !function.sig.generics.params.is_empty() || function.sig.generics.where_clause.is_some() {
        return syn::Error::new_spanned(
            function.sig.generics,
            "firmware hook handlers cannot be generic",
        )
        .to_compile_error()
        .into();
    }
    if function.sig.asyncness.is_some() || function.sig.variadic.is_some() {
        return syn::Error::new_spanned(
            function.sig,
            "firmware hook handlers cannot be async or variadic",
        )
        .to_compile_error()
        .into();
    }
    if !matches!(function.sig.output, ReturnType::Default) {
        return syn::Error::new_spanned(
            function.sig.output,
            "call-site hook handlers currently return `()`",
        )
        .to_compile_error()
        .into();
    }

    let mut names = Vec::new();
    let mut types = Vec::new();
    for arg in &function.sig.inputs {
        let FnArg::Typed(arg) = arg else {
            return syn::Error::new_spanned(arg, "hook handlers must be free functions")
                .to_compile_error()
                .into();
        };
        let Pat::Ident(name) = &*arg.pat else {
            return syn::Error::new_spanned(
                &arg.pat,
                "hook handler arguments must use simple identifier patterns",
            )
            .to_compile_error()
            .into();
        };
        names.push(name.ident.clone());
        types.push((*arg.ty).clone());
    }

    let function_name = args.function;
    let address = match args.at.base10_parse::<u32>() {
        Ok(address) => address,
        Err(error) => return error.to_compile_error().into(),
    };
    let address_literal = LitInt::new(&format!("{address:#010x}"), args.at.span());
    let handler_type_check = format_ident!("__l6_patch_typecheck_{handler}");
    let dynamic_handler = format_ident!("__l6_dynamic_call_{phase}_{handler}");
    let keep_dynamic_handler = format_ident!("__L6_KEEP_DYNAMIC_CALL_{handler}");
    let argument_count = types.len();
    let metadata = format_ident!(
        "__l6_patch_hook_{phase}__{function_name}__{address:08x}__{argument_count}__{handler}"
    );

    function.sig.abi = Some(syn::parse_quote!(extern "C"));
    function.attrs.push(syn::parse_quote!(#[unsafe(no_mangle)]));
    function
        .attrs
        .push(syn::parse_quote!(#[unsafe(link_section = ".text.patch_hook")]));

    let caller_return = address_literal.clone();
    let wrapper_output = quote!(
        <#function_name::Abi as crate::patch_api::FirmwareFunctionOutput>::Output
    );

    quote! {
        #function

        #[allow(dead_code)]
        unsafe fn #handler_type_check(#(#names: #types),*) {
            unsafe { let _ = #function_name(#(#names),*); }
        }

        #[used]
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".patch_hooks")]
        static #metadata: u32 = #address_literal;

        #[cfg(sd_dynamic_module)]
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".text.patch_hook_dynamic")]
        unsafe extern "C" fn #dynamic_handler(
            #(#names: #types),*
        ) -> #wrapper_output {
            let __l6_caller_return: usize;
            unsafe {
                core::arch::asm!(
                    "mov {caller}, lr",
                    caller = out(reg) __l6_caller_return,
                    options(nomem, nostack, preserves_flags),
                );
            }

            let __l6_callsite_return = (#caller_return as usize + 4) | 1;
            if #phase == "pre" && __l6_caller_return == __l6_callsite_return {
                unsafe { #handler(#(#names),*) };
            }

            let __l6_result = unsafe {
                crate::call_original!(#function_name, #(#names),*)
            };

            if #phase == "post" && __l6_caller_return == __l6_callsite_return {
                unsafe { #handler(#(#names),*) };
            }

            __l6_result
        }

        #[cfg(sd_dynamic_module)]
        #[used]
        static #keep_dynamic_handler: unsafe extern "C" fn(
            #(#types),*
        ) -> #wrapper_output = #dynamic_handler;
    }
    .into()
}

#[proc_macro_attribute]
pub fn function_entry_hook(args: TokenStream, input: TokenStream) -> TokenStream {
    let SiteId(site) = parse_macro_input!(args as SiteId);
    let mut function = parse_macro_input!(input as ItemFn);
    let handler = function.sig.ident.clone();
    if let Err(error) = validate_hook_function(&function) {
        return error.to_compile_error().into();
    }

    let inputs = function.sig.inputs.iter().collect::<Vec<_>>();
    let mut names = Vec::with_capacity(inputs.len());
    let mut types = Vec::with_capacity(inputs.len());
    for input in inputs {
        let FnArg::Typed(input) = input else {
            return syn::Error::new_spanned(input, "entry hooks must be free functions")
                .to_compile_error()
                .into();
        };
        let Pat::Ident(name) = &*input.pat else {
            return syn::Error::new_spanned(
                &input.pat,
                "entry hook arguments must use simple identifier patterns",
            )
            .to_compile_error()
            .into();
        };
        names.push(name.ident.clone());
        types.push((*input.ty).clone());
    }
    let result = match &function.sig.output {
        ReturnType::Default => None,
        ReturnType::Type(_, result) => Some((**result).clone()),
    };
    let signature_check = format_ident!("__l6_patch_signature_check_{handler}");
    let body = function.block;
    let metadata = format_ident!("__l6_patch_hook_entry__{site}__{handler}");

    let wrapped_body = if let Some(result) = result {
        quote! {{
            let hook_continue = |#(#names: #types),*| {
                let address: usize = crate::patch_api::__continuations::address(stringify!(#site));
                let continuation: unsafe extern "C" fn(#(#types),*) -> #result =
                    unsafe { core::mem::transmute(address | 1usize) };
                crate::patch_api::EntryHookAction::Continue(
                    unsafe { continuation(#(#names),*) }
                )
            };
            let hook_done = |value: #result| crate::patch_api::EntryHookAction::Done(value);
            let action: crate::patch_api::EntryHookAction<#result> = (|| #body)();
            match action {
                crate::patch_api::EntryHookAction::Continue(value)
                | crate::patch_api::EntryHookAction::Done(value) => value,
            }
        }}
    } else {
        quote! {{
            let hook_continue = |#(#names: #types),*| {
                let address: usize = crate::patch_api::__continuations::address(stringify!(#site));
                let continuation: unsafe extern "C" fn(#(#types),*) =
                    unsafe { core::mem::transmute(address | 1usize) };
                crate::patch_api::EntryHookAction::Continue(
                    unsafe { continuation(#(#names),*) }
                )
            };
            let hook_done = || crate::patch_api::EntryHookAction::Done(());
            let action: crate::patch_api::EntryHookAction<()> = (|| #body)();
            match action {
                crate::patch_api::EntryHookAction::Continue(())
                | crate::patch_api::EntryHookAction::Done(()) => {}
            }
        }}
    };

    function.block = Box::new(syn::parse_quote!(#wrapped_body));
    add_handler_attributes(&mut function, ".text.patch_hook");
    quote! {
        #function

        #[allow(dead_code)]
        fn #signature_check() {
            let _: #site::Abi = #handler;
        }

        #[used]
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".patch_hooks")]
        static #metadata: u32 = 1;
    }
    .into()
}

#[proc_macro_attribute]
pub fn function_offset_hook(args: TokenStream, input: TokenStream) -> TokenStream {
    let args = parse_macro_input!(args as OffsetHookArgs);
    let mut function = parse_macro_input!(input as ItemFn);
    let helper = function.sig.ident.clone();
    if let Err(error) = validate_hook_function(&function) {
        return error.to_compile_error().into();
    }
    if !function.sig.inputs.is_empty() || !matches!(function.sig.output, ReturnType::Default) {
        return syn::Error::new_spanned(
            function.sig,
            "offset hook helpers take no arguments and return `()`",
        )
        .to_compile_error()
        .into();
    }
    let offset = match args.at.base10_parse::<u32>() {
        Ok(offset) => offset,
        Err(error) => return error.to_compile_error().into(),
    };
    let displaced_len = match args.displaced_len.base10_parse::<u16>() {
        Ok(length) if length >= 4 && length & 1 == 0 => length,
        _ => {
            return syn::Error::new_spanned(
                args.displaced_len,
                "displaced_len must be an even value of at least 4",
            )
            .to_compile_error()
            .into();
        }
    };
    if args.guard_sha256.value().len() != 64
        || !args
            .guard_sha256
            .value()
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit())
    {
        return syn::Error::new_spanned(
            args.guard_sha256,
            "guard_sha256 must contain 64 hexadecimal characters",
        )
        .to_compile_error()
        .into();
    }
    let offset_literal = LitInt::new(&format!("{offset:#x}"), args.at.span());
    let site = args.function;
    let metadata = format_ident!(
        "__l6_patch_hook_offset__{site}__{offset:x}__{displaced_len}__{}__{helper}",
        args.guard_sha256.value()
    );
    let body = function.block;
    function.sig.output = syn::parse_quote!(-> u32);
    function.block = Box::new(syn::parse_quote!({
        const _: usize = #site::ADDRESS + #offset_literal;
        let hook_continue = || crate::patch_api::OffsetHookAction::Continue;
        let hook_done = || crate::patch_api::OffsetHookAction::Done;
        let action: crate::patch_api::OffsetHookAction = (|| #body)();
        action as u32
    }));
    let section = format!(".text.{helper}");
    add_handler_attributes(&mut function, &section);
    quote! {
        #function

        #[used]
        #[unsafe(no_mangle)]
        #[unsafe(link_section = ".patch_hooks")]
        static #metadata: u32 = 2;
    }
    .into()
}

fn validate_hook_function(function: &ItemFn) -> syn::Result<()> {
    if !function.sig.generics.params.is_empty() || function.sig.generics.where_clause.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig.generics,
            "firmware hook handlers cannot be generic",
        ));
    }
    if function.sig.asyncness.is_some() || function.sig.variadic.is_some() {
        return Err(syn::Error::new_spanned(
            &function.sig,
            "firmware hook handlers cannot be async or variadic",
        ));
    }
    Ok(())
}

fn add_handler_attributes(function: &mut ItemFn, section: &str) {
    function.sig.abi = Some(syn::parse_quote!(extern "C"));
    function.attrs.push(syn::parse_quote!(#[unsafe(no_mangle)]));
    function
        .attrs
        .push(syn::parse_quote!(#[unsafe(link_section = #section)]));
}
