use proc_macro::TokenStream;
use quote::{format_ident, quote};
use syn::{
    Attribute, Expr, ExprLit, FnArg, ImplItem, ImplItemFn, ItemImpl, ItemStruct, Lit, Meta, Path, Result, Token, Type,
    parenthesized,
    parse::{Parse, ParseStream},
    parse_macro_input,
};

#[proc_macro_derive(StdlibModule, attributes(stdlib_module))]
pub fn derive_stdlib_module(input: TokenStream) -> TokenStream {
    let item = parse_macro_input!(input as ItemStruct);
    match expand_stdlib_module(item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

#[proc_macro_attribute]
pub fn stdlib_exports(attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut impl_item = parse_macro_input!(item as ItemImpl);
    let args = parse_macro_input!(attr as StdlibExportsArgs);
    match expand_stdlib_exports(args, &mut impl_item) {
        Ok(tokens) => tokens.into(),
        Err(err) => err.to_compile_error().into(),
    }
}

fn expand_stdlib_module(item: ItemStruct) -> Result<proc_macro2::TokenStream> {
    let ident = item.ident;
    let args = parse_module_attr(&item.attrs)?;
    let name = args
        .name
        .ok_or_else(|| syn::Error::new_spanned(&ident, "missing #[stdlib_module(name = \"...\")]"))?;
    let docs = args.docs.unwrap_or_default();

    Ok(quote! {
        impl #ident {
            pub const fn stdlib_module_name() -> &'static str {
                #name
            }

            pub const fn stdlib_module_docs() -> Option<&'static str> {
                if #docs.is_empty() {
                    None
                } else {
                    Some(#docs)
                }
            }

            pub fn new() -> Self {
                Self::default()
            }
        }
    })
}

fn expand_stdlib_exports(args: StdlibExportsArgs, impl_item: &mut ItemImpl) -> Result<proc_macro2::TokenStream> {
    let self_ty = impl_item.self_ty.as_ref().clone();
    let module_ident = args.module.unwrap_or(module_ident_for_type(&self_ty)?);
    let is_nested_module = module_ident.contains('.');
    let metadata_fn = format_ident!("metadata");
    let register_fn = format_ident!("register");
    let mut exports = Vec::new();
    let mut metadata_exports = Vec::new();
    let mut wrapper_functions = Vec::new();
    let mut value_exports = Vec::new();

    for item in &mut impl_item.items {
        let ImplItem::Fn(function) = item else {
            continue;
        };
        let Some(export) = take_export_attr(function)? else {
            continue;
        };
        let fn_ident = &function.sig.ident;
        let name = export.name.unwrap_or_else(|| export_name_for_function(fn_ident));
        let params = export
            .params
            .ok_or_else(|| syn::Error::new_spanned(fn_ident, "missing export params(...)"))?;
        let arity = params.arity();
        let arity_tokens = arity.tokens();
        let returns = export
            .returns
            .ok_or_else(|| syn::Error::new_spanned(fn_ident, "missing export returns = Type"))?;
        let display_name = format!("{module_ident}.{name}");
        let signature = params.signature(&display_name, &returns.display);
        let docs = export.docs.or_else(|| doc_comments(&function.attrs));
        let kind = export.kind.unwrap_or(ExportKind::Plain);
        let target = if let Some(function) = export.function {
            ExportTarget {
                function_path: quote!(#function),
                wrapper: None,
            }
        } else {
            export_function_path(&self_ty, function, &params, &export.named, &display_name)?
        };
        if let Some(wrapper) = target.wrapper {
            wrapper_functions.push(wrapper);
        }

        let native_ctor = match kind {
            ExportKind::Plain => quote!(::lk_core::module::RuntimeNativeExport::plain),
            ExportKind::FullState => quote!(::lk_core::module::RuntimeNativeExport::full_state),
        };
        exports.push(ExportExpansion {
            name: name.clone(),
            function_path: target.function_path,
            arity_tokens,
            native_ctor,
            kind,
        });
        let docs_tokens = option_str_tokens(docs.as_deref());
        let return_kind = &returns.kind;
        metadata_exports.push(quote! {
            ::lk_stdlib_common::metadata::StdlibCallableMetadata::new(
                concat!(#module_ident, ".", #name),
                concat!(#module_ident, ".", #name),
                ::lk_stdlib_common::metadata::StdlibReturnKind::#return_kind,
                Some(#signature),
                #docs_tokens,
            )
        });
    }

    for attr in impl_item.attrs.iter() {
        if !attr.path().is_ident("stdlib_export") {
            continue;
        }
        let export = parse_export_args(attr)?;
        let function_path = export
            .function
            .ok_or_else(|| syn::Error::new_spanned(attr, "impl-level stdlib_export requires function = path"))?;
        let name = export
            .name
            .ok_or_else(|| syn::Error::new_spanned(attr, "impl-level stdlib_export requires name = \"...\""))?;
        let params = export
            .params
            .ok_or_else(|| syn::Error::new_spanned(attr, "impl-level stdlib_export requires params(...)"))?;
        let arity = params.arity();
        let arity_tokens = arity.tokens();
        let returns = export
            .returns
            .ok_or_else(|| syn::Error::new_spanned(attr, "impl-level stdlib_export requires returns = Type"))?;
        let display_name = format!("{module_ident}.{name}");
        let signature = params.signature(&display_name, &returns.display);
        let kind = export.kind.unwrap_or(ExportKind::Plain);
        let native_ctor = match kind {
            ExportKind::Plain => quote!(::lk_core::module::RuntimeNativeExport::plain),
            ExportKind::FullState => quote!(::lk_core::module::RuntimeNativeExport::full_state),
        };
        exports.push(ExportExpansion {
            name: name.clone(),
            function_path: quote!(#function_path),
            arity_tokens,
            native_ctor,
            kind,
        });
        let docs_tokens = option_str_tokens(export.docs.as_deref());
        let return_kind = &returns.kind;
        metadata_exports.push(quote! {
            ::lk_stdlib_common::metadata::StdlibCallableMetadata::new(
                concat!(#module_ident, ".", #name),
                concat!(#module_ident, ".", #name),
                ::lk_stdlib_common::metadata::StdlibReturnKind::#return_kind,
                Some(#signature),
                #docs_tokens,
            )
        });
    }
    impl_item.attrs.retain(|attr| !attr.path().is_ident("stdlib_export"));

    for attr in impl_item.attrs.iter() {
        if !attr.path().is_ident("stdlib_value") {
            continue;
        }
        let value = attr.parse_args::<ValueExport>()?;
        let name = value.name;
        let expr = value.expr;
        value_exports.push(quote! {
            ::lk_core::module::RuntimeValueExport::new(#name, #expr)
        });
    }
    impl_item.attrs.retain(|attr| !attr.path().is_ident("stdlib_value"));

    let native_exports = exports.iter().map(|export| {
        let name = &export.name;
        let function_path = &export.function_path;
        let arity_tokens = &export.arity_tokens;
        let native_ctor = &export.native_ctor;
        quote!(#native_ctor(#name, #function_path, #arity_tokens))
    });
    let runtime_builtin_exports = exports.iter().map(|export| {
        let builtin_name = format!("{module_ident}::{}", export.name);
        let function_path = &export.function_path;
        let arity_tokens = &export.arity_tokens;
        let function_ctor = match export.kind {
            ExportKind::Plain => quote!(::lk_core::vm::NativeFunction::Plain),
            ExportKind::FullState => quote!(::lk_core::vm::NativeFunction::FullState),
        };
        quote! {
            registry.register_runtime_builtin(#builtin_name, #function_ctor(#function_path), #arity_tokens);
        }
    });
    let runtime_builtin_tokens = if args.runtime_builtins {
        quote! {
            #(#runtime_builtin_exports)*
        }
    } else {
        quote!()
    };
    let child_namespace_exports = args.children.iter().map(|child| {
        let name = &child.name;
        let module_type = &child.module_type;
        quote! {
            (
                #name,
                ::lk_core::module::ModuleProvider::runtime_exports(&<#module_type>::new())?,
            )
        }
    });
    let child_metadata_registers = args.children.iter().map(|child| {
        let module_type = &child.module_type;
        quote! {
            ::lk_stdlib_common::metadata::register_stdlib_module_metadata(<#module_type>::stdlib_metadata())?;
        }
    });
    let module_functions = if is_nested_module {
        quote!()
    } else {
        quote! {
            pub fn #register_fn(registry: &mut ::lk_core::module::ModuleRegistry) -> ::anyhow::Result<()> {
                ::lk_stdlib_common::metadata::register_stdlib_module_metadata(#metadata_fn())?;
                #(#child_metadata_registers)*
                registry.register_module(<#self_ty>::stdlib_module_name(), Box::new(<#self_ty>::new()))
            }

            pub fn #metadata_fn() -> ::lk_stdlib_common::metadata::StdlibModuleMetadata {
                <#self_ty>::stdlib_metadata()
            }
        }
    };
    let expanded_impl = quote!(#impl_item);
    Ok(quote! {
        #expanded_impl
        impl #self_ty {
            #(#wrapper_functions)*

            pub fn stdlib_metadata() -> ::lk_stdlib_common::metadata::StdlibModuleMetadata {
                const CALLABLES: &[::lk_stdlib_common::metadata::StdlibCallableMetadata] = &[
                    #(#metadata_exports),*
                ];
                ::lk_stdlib_common::metadata::StdlibModuleMetadata::new(
                    #module_ident,
                    <#self_ty>::stdlib_module_docs(),
                    CALLABLES,
                )
            }
        }

        impl ::lk_core::module::ModuleProvider for #self_ty {
            fn name(&self) -> &str {
                <#self_ty>::stdlib_module_name()
            }

            fn description(&self) -> &str {
                <#self_ty>::stdlib_module_docs().unwrap_or("stdlib module")
            }

            fn register(&self, registry: &mut ::lk_core::module::ModuleRegistry) -> ::anyhow::Result<()> {
                #runtime_builtin_tokens
                Ok(())
            }

            fn runtime_exports(&self) -> ::anyhow::Result<::lk_core::vm::RuntimeExport> {
                ::lk_stdlib_common::runtime_native::module_export(
                    &[#(#native_exports),*],
                    &[#(#value_exports),*],
                    &[#(#child_namespace_exports),*],
                )
            }
        }

        #module_functions
    })
}

#[derive(Default)]
struct ModuleArgs {
    name: Option<String>,
    docs: Option<String>,
}

#[derive(Default)]
struct StdlibExportsArgs {
    module: Option<String>,
    runtime_builtins: bool,
    children: Vec<ChildModule>,
}

struct ChildModule {
    name: String,
    module_type: Path,
}

struct ExportExpansion {
    name: String,
    function_path: proc_macro2::TokenStream,
    arity_tokens: proc_macro2::TokenStream,
    native_ctor: proc_macro2::TokenStream,
    kind: ExportKind,
}

struct ExportTarget {
    function_path: proc_macro2::TokenStream,
    wrapper: Option<proc_macro2::TokenStream>,
}

impl Parse for StdlibExportsArgs {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut args = Self::default();
        while !input.is_empty() {
            let ident: syn::Ident = input.parse()?;
            if ident == "children" {
                let content;
                parenthesized!(content in input);
                while !content.is_empty() {
                    let name = content.parse::<syn::Ident>()?.to_string();
                    content.parse::<Token![=]>()?;
                    let module_type = content.parse::<Path>()?;
                    args.children.push(ChildModule { name, module_type });
                    if content.peek(Token![,]) {
                        content.parse::<Token![,]>()?;
                    }
                }
            } else if ident == "module" {
                input.parse::<Token![=]>()?;
                args.module = Some(input.parse::<Lit>()?.expect_string()?);
            } else if ident == "runtime_builtins" {
                input.parse::<Token![=]>()?;
                args.runtime_builtins = input.parse::<Lit>()?.expect_bool()?;
            } else {
                return Err(syn::Error::new_spanned(ident, "unsupported stdlib_exports option"));
            }
            if input.peek(Token![,]) {
                input.parse::<Token![,]>()?;
            }
        }
        Ok(args)
    }
}

fn parse_module_attr(attrs: &[Attribute]) -> Result<ModuleArgs> {
    let mut out = ModuleArgs::default();
    for attr in attrs {
        if !attr.path().is_ident("stdlib_module") {
            continue;
        }
        attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("name") {
                out.name = Some(meta.value()?.parse::<Lit>()?.expect_string()?);
                Ok(())
            } else if meta.path.is_ident("docs") {
                out.docs = Some(meta.value()?.parse::<Lit>()?.expect_string()?);
                Ok(())
            } else {
                Err(meta.error("unsupported stdlib_module option"))
            }
        })?;
    }
    Ok(out)
}

fn export_function_path(
    self_ty: &Type,
    function: &ImplItemFn,
    params: &ParamList,
    named: &[String],
    display_name: &str,
) -> Result<ExportTarget> {
    let fn_ident = &function.sig.ident;
    let wrapper_ident = format_ident!("__lk_stdlib_export_{}", fn_ident);
    let precheck = params.precheck_tokens(named, display_name);
    if is_raw_native_function(function) {
        let wrapper = quote! {
            fn #wrapper_ident(
                args: ::lk_core::vm::NativeArgs<'_>,
                runtime: &mut ::lk_core::vm::NativeRuntime<'_>,
            ) -> ::anyhow::Result<::lk_core::val::RuntimeVal> {
                #precheck
                <Self>::#fn_ident(args, runtime)
            }
        };
        return Ok(ExportTarget {
            function_path: quote!(#self_ty::#wrapper_ident),
            wrapper: Some(wrapper),
        });
    }
    let arity = params.arity();
    let Arity::Fixed(expected_arity) = arity else {
        return Err(syn::Error::new_spanned(
            fn_ident,
            "variadic stdlib_export functions must use raw NativeArgs ABI",
        ));
    };

    let mut positional_count = 0u16;
    let mut call_args = Vec::new();
    for input in &function.sig.inputs {
        let FnArg::Typed(arg) = input else {
            return Err(syn::Error::new_spanned(
                input,
                "stdlib_export methods must not take self",
            ));
        };
        let ty = arg.ty.as_ref();
        if is_mut_native_runtime(ty) {
            call_args.push(quote!(runtime));
        } else if is_runtime_val(ty) {
            let idx = positional_count as usize;
            call_args.push(quote!(values[#idx].clone()));
            positional_count += 1;
        } else if is_ref_runtime_val(ty) {
            let idx = positional_count as usize;
            call_args.push(quote!(&values[#idx]));
            positional_count += 1;
        } else {
            return Err(syn::Error::new_spanned(
                ty,
                "ergonomic stdlib_export parameters must be RuntimeVal, &RuntimeVal, or &mut NativeRuntime",
            ));
        }
    }
    if positional_count != expected_arity {
        return Err(syn::Error::new_spanned(
            fn_ident,
            format!(
                "stdlib_export fixed arity is {expected_arity}, but function has {positional_count} value parameters"
            ),
        ));
    }
    let wrapper = quote! {
        fn #wrapper_ident(
            args: ::lk_core::vm::NativeArgs<'_>,
            runtime: &mut ::lk_core::vm::NativeRuntime<'_>,
        ) -> ::anyhow::Result<::lk_core::val::RuntimeVal> {
            #precheck
            let values = args.as_slice();
            <Self>::#fn_ident(#(#call_args),*)
        }
    };
    Ok(ExportTarget {
        function_path: quote!(#self_ty::#wrapper_ident),
        wrapper: Some(wrapper),
    })
}

fn is_raw_native_function(function: &ImplItemFn) -> bool {
    let mut inputs = function.sig.inputs.iter();
    let Some(FnArg::Typed(first)) = inputs.next() else {
        return false;
    };
    let Some(FnArg::Typed(second)) = inputs.next() else {
        return false;
    };
    inputs.next().is_none() && is_native_args(first.ty.as_ref()) && is_mut_native_runtime(second.ty.as_ref())
}

#[derive(Default)]
struct ExportArgs {
    name: Option<String>,
    function: Option<Path>,
    params: Option<ParamList>,
    returns: Option<ReturnSpec>,
    kind: Option<ExportKind>,
    named: Vec<String>,
    docs: Option<String>,
}

fn take_export_attr(function: &mut ImplItemFn) -> Result<Option<ExportArgs>> {
    let mut found = None;
    let mut attrs = Vec::with_capacity(function.attrs.len());
    for attr in function.attrs.drain(..) {
        if attr.path().is_ident("export") || attr.path().is_ident("stdlib_export") {
            found = Some(parse_export_args(&attr)?);
        } else {
            attrs.push(attr);
        }
    }
    function.attrs = attrs;
    Ok(found)
}

fn parse_export_args(attr: &Attribute) -> Result<ExportArgs> {
    let mut args = ExportArgs::default();
    attr.parse_nested_meta(|meta| {
        if meta.path.is_ident("name") {
            args.name = Some(meta.value()?.parse::<Lit>()?.expect_string()?);
            Ok(())
        } else if meta.path.is_ident("function") {
            args.function = Some(meta.value()?.parse::<Path>()?);
            Ok(())
        } else if meta.path.is_ident("params") {
            let content;
            parenthesized!(content in meta.input);
            let tokens: proc_macro2::TokenStream = content.parse()?;
            args.params = Some(ParamList::parse_tokens(tokens)?);
            Ok(())
        } else if meta.path.is_ident("returns") {
            let tokens = meta.value()?.parse::<ReturnTokens>()?.0;
            args.returns = Some(ReturnSpec::parse_tokens(tokens));
            Ok(())
        } else if meta.path.is_ident("kind") {
            args.kind = Some(meta.value()?.parse::<Lit>()?.expect_kind()?);
            Ok(())
        } else if meta.path.is_ident("named") {
            let content;
            parenthesized!(content in meta.input);
            while !content.is_empty() {
                args.named.push(content.parse::<syn::Ident>()?.to_string());
                if content.peek(Token![,]) {
                    content.parse::<Token![,]>()?;
                }
            }
            Ok(())
        } else if meta.path.is_ident("docs") {
            args.docs = Some(meta.value()?.parse::<Lit>()?.expect_string()?);
            Ok(())
        } else if meta.path.is_ident("arity") || meta.path.is_ident("signature") {
            Err(meta.error("stdlib_export uses params(...) instead of arity/signature"))
        } else {
            Err(meta.error("unsupported export option"))
        }
    })?;
    Ok(args)
}

#[derive(Debug, Clone)]
struct ParamList {
    signatures: Vec<ParamSignature>,
}

#[derive(Debug, Clone)]
struct ParamSignature {
    params: Vec<ParamSpec>,
}

#[derive(Debug, Clone)]
struct ParamSpec {
    name: String,
    ty: String,
    optional: bool,
    variadic: bool,
    default: Option<String>,
}

#[derive(Debug, Clone)]
struct ReturnSpec {
    kind: syn::Ident,
    display: String,
}

struct ReturnTokens(proc_macro2::TokenStream);

impl Parse for ReturnTokens {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let mut tokens = proc_macro2::TokenStream::new();
        while !input.is_empty() && !input.peek(Token![,]) {
            let token: proc_macro2::TokenTree = input.parse()?;
            tokens.extend([token]);
        }
        Ok(Self(tokens))
    }
}

impl ParamList {
    fn parse_tokens(tokens: proc_macro2::TokenStream) -> Result<Self> {
        let source = tokens.to_string();
        let signatures = split_top_level(&source, ';')
            .into_iter()
            .map(|signature| ParamSignature::parse(&signature))
            .collect::<Result<Vec<_>>>()?;
        if signatures.is_empty() {
            return Ok(Self {
                signatures: vec![ParamSignature { params: Vec::new() }],
            });
        }
        Ok(Self { signatures })
    }

    fn arity(&self) -> Arity {
        if self.signatures.len() != 1 {
            return Arity::Variadic;
        }
        let signature = &self.signatures[0];
        if signature
            .params
            .iter()
            .any(|param| param.optional || param.variadic || param.default.is_some())
        {
            Arity::Variadic
        } else {
            Arity::Fixed(signature.params.len() as u16)
        }
    }

    fn signature(&self, name: &str, returns: &str) -> String {
        self.signatures
            .iter()
            .map(|signature| format!("{name}({}) -> {returns}", signature.display()))
            .collect::<Vec<_>>()
            .join(" | ")
    }

    fn precheck_tokens(&self, named: &[String], display_name: &str) -> proc_macro2::TokenStream {
        let has_named = !named.is_empty();
        let checks = self.signatures.iter().map(|signature| {
            let min = signature
                .params
                .iter()
                .filter(|param| !param.optional && !param.variadic)
                .count();
            let variadic = signature.params.iter().any(|param| param.variadic);
            let max = signature.params.len();
            // Shape the generated comparison so expanded code stays
            // clippy-clean (`double_comparisons`, `manual_range_contains`).
            if has_named {
                if variadic {
                    quote!(true)
                } else {
                    quote!((__lk_stdlib_export_arg_len <= #max))
                }
            } else if variadic {
                quote!((__lk_stdlib_export_arg_len >= #min))
            } else if min == max {
                quote!((__lk_stdlib_export_arg_len == #max))
            } else if min == 0 {
                quote!((__lk_stdlib_export_arg_len <= #max))
            } else {
                quote!(((#min..=#max).contains(&__lk_stdlib_export_arg_len)))
            }
        });
        let expected = self.argument_count_description(has_named);
        let named = named
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>();
        let named_checks = named.iter().map(|name| quote!(#name));
        let named_tokens = if named.is_empty() {
            quote! {
                if args.has_named() {
                    ::anyhow::bail!("stdlib export does not accept named arguments");
                }
            }
        } else {
            quote! {
                args.try_for_each_named(runtime.heap(), |__lk_stdlib_export_name, _| {
                    match __lk_stdlib_export_name {
                        #(#named_checks)|* => Ok(()),
                        other => ::anyhow::bail!("stdlib export does not accept named argument '{}'", other),
                    }
                })?;
            }
        };
        quote! {
            let __lk_stdlib_export_arg_len = args.len();
            if !(#(#checks)||*) {
                ::anyhow::bail!("{} takes {}", #display_name, #expected);
            }
            #named_tokens
        }
    }

    fn argument_count_description(&self, named: bool) -> String {
        let ranges = self
            .signatures
            .iter()
            .map(|signature| {
                let min = signature
                    .params
                    .iter()
                    .filter(|param| !param.optional && !param.variadic)
                    .count();
                let max = if signature.params.iter().any(|param| param.variadic) {
                    None
                } else {
                    Some(signature.params.len())
                };
                let effective_min = if named { 0 } else { min };
                (effective_min, max)
            })
            .collect::<Vec<_>>();

        if ranges.len() == 1 {
            let (min, max) = ranges[0];
            return match max {
                Some(max) if min == max => format!("exactly {min} {}", plural_argument(min)),
                Some(max) => format!("{min} to {max} arguments"),
                None if min == 0 => "any number of arguments".to_string(),
                None => format!("at least {min} {}", plural_argument(min)),
            };
        }

        let parts = ranges
            .into_iter()
            .map(|(min, max)| match max {
                Some(max) if min == max => min.to_string(),
                Some(max) => format!("{min}-{max}"),
                None => format!("{min}+"),
            })
            .collect::<Vec<_>>()
            .join(" or ");
        format!("{parts} arguments")
    }
}

fn plural_argument(count: usize) -> &'static str {
    if count == 1 { "argument" } else { "arguments" }
}

impl ParamSignature {
    fn parse(source: &str) -> Result<Self> {
        let source = source.trim();
        if source.is_empty() {
            return Ok(Self { params: Vec::new() });
        }
        let params = split_top_level(source, ',')
            .into_iter()
            .map(|param| ParamSpec::parse(&param))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self { params })
    }

    fn display(&self) -> String {
        self.params
            .iter()
            .map(ParamSpec::display)
            .collect::<Vec<_>>()
            .join(", ")
    }
}

impl ParamSpec {
    fn parse(source: &str) -> Result<Self> {
        let mut source = source.trim();
        let variadic = source.starts_with("...");
        if variadic {
            source = source.trim_start_matches('.').trim();
        }
        let Some(colon_idx) = source.find(':') else {
            return Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("stdlib_export param '{source}' is missing ':'"),
            ));
        };
        let mut name = source[..colon_idx].trim().to_string();
        let optional = name.ends_with('?');
        if optional {
            name.pop();
            name = name.trim_end().to_string();
        }
        let rest = source[colon_idx + 1..].trim();
        let (ty, default) = split_default(rest);
        Ok(Self {
            name,
            ty: normalize_type_display(&ty),
            optional,
            variadic,
            default: default.as_deref().map(normalize_type_display),
        })
    }

    fn display(&self) -> String {
        let mut out = String::new();
        if self.variadic {
            out.push_str("...");
        }
        out.push_str(&self.name);
        if self.optional {
            out.push('?');
        }
        out.push_str(": ");
        out.push_str(&self.ty);
        if let Some(default) = &self.default {
            out.push_str(" = ");
            out.push_str(default);
        }
        out
    }
}

impl ReturnSpec {
    fn parse_tokens(tokens: proc_macro2::TokenStream) -> Self {
        let display = normalize_type_display(&tokens.to_string());
        let kind_name = match display.as_str() {
            "Nil" => "Nil",
            "Bool" => "Bool",
            "Int" => "Int",
            "IntOrFloat" | "Int | Float" => "IntOrFloat",
            "Float" => "Float",
            "String" => "String",
            _ => "RuntimeValue",
        };
        Self {
            kind: format_ident!("{kind_name}"),
            display,
        }
    }
}

fn split_default(source: &str) -> (String, Option<String>) {
    let mut depth = 0i32;
    for (idx, ch) in source.char_indices() {
        match ch {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            '=' if depth == 0 => {
                return (
                    source[..idx].trim().to_string(),
                    Some(source[idx + 1..].trim().to_string()),
                );
            }
            _ => {}
        }
    }
    (source.trim().to_string(), None)
}

fn split_top_level(source: &str, separator: char) -> Vec<String> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    for (idx, ch) in source.char_indices() {
        match ch {
            '<' | '[' | '(' => depth += 1,
            '>' | ']' | ')' => depth -= 1,
            _ if ch == separator && depth == 0 => {
                let item = source[start..idx].trim();
                if !item.is_empty() {
                    out.push(item.to_string());
                }
                start = idx + ch.len_utf8();
            }
            _ => {}
        }
    }
    let item = source[start..].trim();
    if !item.is_empty() {
        out.push(item.to_string());
    }
    out
}

fn normalize_type_display(source: &str) -> String {
    source
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .replace(" < ", "<")
        .replace("< ", "<")
        .replace(" >", ">")
        .replace(" , ", ", ")
        .replace("[ ", "[")
        .replace(" ]", "]")
        .replace(" ? ", "?")
        .replace(" ?", "?")
}

#[derive(Debug, Clone, Copy)]
enum Arity {
    Fixed(u16),
    Variadic,
}

impl Arity {
    fn tokens(self) -> proc_macro2::TokenStream {
        match self {
            Self::Fixed(value) => quote!(#value),
            Self::Variadic => quote!(::lk_core::vm::NativeEntry::VARIADIC),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum ExportKind {
    Plain,
    FullState,
}

fn is_native_args(ty: &Type) -> bool {
    type_path_last_ident(ty).is_some_and(|ident| ident == "NativeArgs")
}

fn is_mut_native_runtime(ty: &Type) -> bool {
    let Type::Reference(reference) = ty else {
        return false;
    };
    reference.mutability.is_some()
        && type_path_last_ident(reference.elem.as_ref()).is_some_and(|ident| ident == "NativeRuntime")
}

fn is_runtime_val(ty: &Type) -> bool {
    type_path_last_ident(ty).is_some_and(|ident| ident == "RuntimeVal")
}

fn is_ref_runtime_val(ty: &Type) -> bool {
    let Type::Reference(reference) = ty else {
        return false;
    };
    reference.mutability.is_none() && is_runtime_val(reference.elem.as_ref())
}

fn type_path_last_ident(ty: &Type) -> Option<String> {
    let Type::Path(path) = ty else {
        return None;
    };
    path.path.segments.last().map(|segment| segment.ident.to_string())
}

trait LitExt {
    fn expect_string(self) -> Result<String>;
    fn expect_bool(self) -> Result<bool>;
    fn expect_kind(self) -> Result<ExportKind>;
}

impl LitExt for Lit {
    fn expect_string(self) -> Result<String> {
        match self {
            Lit::Str(value) => Ok(value.value()),
            other => Err(syn::Error::new_spanned(other, "expected string literal")),
        }
    }

    fn expect_bool(self) -> Result<bool> {
        match self {
            Lit::Bool(value) => Ok(value.value),
            other => Err(syn::Error::new_spanned(other, "expected bool literal")),
        }
    }

    fn expect_kind(self) -> Result<ExportKind> {
        match self.expect_string()?.as_str() {
            "plain" => Ok(ExportKind::Plain),
            "full_state" => Ok(ExportKind::FullState),
            other => Err(syn::Error::new(
                proc_macro2::Span::call_site(),
                format!("unsupported export kind '{other}'"),
            )),
        }
    }
}

struct ValueExport {
    name: String,
    expr: Expr,
}

impl Parse for ValueExport {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let name_lit: Lit = input.parse()?;
        input.parse::<Token![=>]>()?;
        let expr = input.parse()?;
        Ok(Self {
            name: name_lit.expect_string()?,
            expr,
        })
    }
}

fn module_ident_for_type(ty: &Type) -> Result<String> {
    let type_name = match ty {
        Type::Path(path) => path
            .path
            .segments
            .last()
            .map(|segment| segment.ident.to_string())
            .ok_or_else(|| syn::Error::new_spanned(ty, "unsupported impl self type")),
        _ => Err(syn::Error::new_spanned(ty, "unsupported impl self type")),
    }?;
    Ok(infer_module_name_from_type(&type_name))
}

fn infer_module_name_from_type(type_name: &str) -> String {
    let stem = type_name.strip_suffix("Module").unwrap_or(type_name);
    let mut out = String::new();
    for (idx, ch) in stem.chars().enumerate() {
        if ch.is_ascii_uppercase() {
            if idx > 0 {
                out.push('_');
            }
            out.push(ch.to_ascii_lowercase());
        } else {
            out.push(ch);
        }
    }
    out
}

fn export_name_for_function(ident: &syn::Ident) -> String {
    let name = ident.to_string();
    name.strip_suffix("_export").unwrap_or(&name).to_string()
}

fn doc_comments(attrs: &[Attribute]) -> Option<String> {
    let mut lines = Vec::new();
    for attr in attrs {
        if !attr.path().is_ident("doc") {
            continue;
        }
        if let Meta::NameValue(name_value) = &attr.meta
            && let Expr::Lit(ExprLit {
                lit: Lit::Str(value), ..
            }) = &name_value.value
        {
            lines.push(value.value().trim().to_string());
        }
    }
    let docs = lines.join("\n").trim().to_string();
    (!docs.is_empty()).then_some(docs)
}

fn option_str_tokens(value: Option<&str>) -> proc_macro2::TokenStream {
    match value {
        Some(value) => quote!(Some(#value)),
        None => quote!(None),
    }
}
