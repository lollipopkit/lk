// The parser's AST hands statements around as `Vec<Box<Stmt>>`; keeping the
// macro system on the same shape avoids re-boxing at every parse boundary.
#![allow(clippy::vec_box, clippy::boxed_local)]
use super::{
    AstGeneratedItemOrigin, AstMacroOrigin, AstMacroOriginKind, AstMacroState, BUILTIN_SHOW_TRAIT,
    PROC_MACRO_PROTOCOL_VERSION, ProcMacroKind, ProcMacroOptions, ProcMacroRequest, ProcMacroToken,
    apply_preserved_attributes, error_at_attr, error_from_span, origins,
    origins::{builtin_show_generated_member_origins, stmt_label},
    parse_proc_macro_output_items, record_ast_origin, reject_error_diagnostics, run_proc_macro_process,
    stmt_item_tokens,
};
use crate::{
    expr::{Expr, TemplateStringPart},
    stmt::{Attribute, Stmt},
    token::{ParseError, Span, Token},
    val::{LiteralVal, Type},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum DeriveMacro {
    Debug,
    Show,
    External(String),
}

impl DeriveMacro {
    fn is_builtin_show(&self) -> bool {
        matches!(self, Self::Debug | Self::Show)
    }

    fn external_name(&self) -> Option<&str> {
        match self {
            Self::External(name) => Some(name),
            _ => None,
        }
    }
}

pub(super) fn parse_derive_attribute(attr: &Attribute) -> Result<Option<Vec<DeriveMacro>>, ParseError> {
    let Some(Token::Id(name)) = attr.tokens.first() else {
        return Ok(None);
    };
    if name != "derive" {
        return Ok(None);
    }
    if attr.tokens.len() < 3 || attr.tokens.get(1) != Some(&Token::LParen) || attr.tokens.last() != Some(&Token::RParen)
    {
        return Err(error_at_attr(
            attr,
            "Malformed derive attribute; expected #[derive(Name, ...)]",
        ));
    }

    let mut derives = Vec::new();
    let mut pos = 2;
    while pos + 1 < attr.tokens.len() {
        match &attr.tokens[pos] {
            Token::Id(name) => {
                derives.push(parse_derive_name(name));
                pos += 1;
            }
            Token::RParen => break,
            _ => return Err(error_at_attr(attr, "Expected derive macro name")),
        }

        match attr.tokens.get(pos) {
            Some(Token::Comma) => {
                pos += 1;
                if attr.tokens.get(pos) == Some(&Token::RParen) {
                    break;
                }
            }
            Some(Token::RParen) => break,
            _ => return Err(error_at_attr(attr, "Expected ',' or ')' in derive attribute")),
        }
    }

    if derives.is_empty() {
        return Err(error_at_attr(attr, "derive attribute requires at least one macro name"));
    }
    Ok(Some(derives))
}

pub(super) fn expand_derives(
    derives: Vec<DeriveMacro>,
    derive_span: Option<Span>,
    preserved_attrs: Vec<Attribute>,
    expanded_item: Stmt,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    let needs_builtin_show = derives.iter().any(DeriveMacro::is_builtin_show);
    let has_external_derive = derives.iter().any(|derive| derive.external_name().is_some());

    let mut output = Vec::new();
    let mut builtin_generated = Vec::new();
    if needs_builtin_show && !state.show_trait_available {
        builtin_generated.push(Box::new(builtin_show_trait()));
        state.show_trait_available = true;
    }

    if needs_builtin_show {
        let Stmt::Struct { name, fields } = &expanded_item else {
            return Err(error_from_span(
                derive_span.as_ref(),
                "built-in derive macros currently support structs only",
            ));
        };
        builtin_generated.push(Box::new(derive_show_impl(name, fields)));
        let macro_name = builtin_derive_origin_name(&derives);
        record_builtin_show_ast_origin(state, &macro_name, derive_span.clone(), &builtin_generated, fields);
    }
    if has_external_derive && !is_external_derive_input(&expanded_item) {
        return Err(error_from_span(
            derive_span.as_ref(),
            "external derive macros currently support structs, type aliases, and traits only",
        ));
    }

    output.push(Box::new(apply_preserved_attributes(
        preserved_attrs,
        expanded_item.clone(),
    )));

    if needs_builtin_show {
        output.extend(builtin_generated);
    }
    for derive in derives.iter().filter_map(DeriveMacro::external_name) {
        let generated = expand_external_derive(derive, &expanded_item, options, derive_span.as_ref())?;
        record_ast_origin(
            state,
            derive,
            AstMacroOriginKind::ExternalDerive,
            derive_span.clone(),
            &generated,
        );
        output.extend(generated);
    }
    Ok(output)
}

fn parse_derive_name(name: &str) -> DeriveMacro {
    match name {
        "Debug" => DeriveMacro::Debug,
        "Show" => DeriveMacro::Show,
        _ => DeriveMacro::External(name.to_string()),
    }
}

fn is_external_derive_input(item: &Stmt) -> bool {
    matches!(item, Stmt::Struct { .. } | Stmt::TypeAlias { .. } | Stmt::Trait { .. })
}

fn builtin_derive_origin_name(derives: &[DeriveMacro]) -> String {
    let mut names = derives
        .iter()
        .filter_map(|derive| match derive {
            DeriveMacro::Debug => Some("Debug"),
            DeriveMacro::Show => Some("Show"),
            DeriveMacro::External(_) => None,
        })
        .collect::<Vec<_>>();
    names.sort_unstable();
    names.dedup();
    names.join("+")
}

fn record_builtin_show_ast_origin(
    state: &mut AstMacroState,
    macro_name: &str,
    input_span: Option<Span>,
    generated_items: &[Box<Stmt>],
    fields: &[(String, Option<Type>)],
) {
    let generated_item_origins = generated_items
        .iter()
        .map(|stmt| {
            let label = stmt_label(stmt);
            AstGeneratedItemOrigin {
                generated_member_origins: builtin_show_generated_member_origins(&label, input_span.clone(), fields),
                label,
                span: origins::stmt_span(stmt).cloned().or_else(|| input_span.clone()),
            }
        })
        .collect();
    state.origins.push(AstMacroOrigin {
        macro_name: macro_name.to_string(),
        kind: AstMacroOriginKind::BuiltinDerive,
        input_span,
        generated_items: generated_items.len(),
        generated_item_labels: generated_items.iter().map(|stmt| stmt_label(stmt)).collect(),
        generated_item_origins,
    });
}

fn builtin_show_trait() -> Stmt {
    Stmt::Trait {
        name: BUILTIN_SHOW_TRAIT.to_string(),
        methods: vec![(
            "show".to_string(),
            Type::Function {
                params: vec![Type::Any],
                named_params: Vec::new(),
                return_type: Box::new(Type::String),
            },
        )],
    }
}

fn derive_show_impl(name: &str, fields: &[(String, Option<Type>)]) -> Stmt {
    Stmt::Impl {
        trait_name: BUILTIN_SHOW_TRAIT.to_string(),
        target_type: Type::Named(name.to_string()),
        methods: vec![Stmt::Function {
            name: "show".to_string(),
            params: vec!["self".to_string()],
            param_types: vec![Some(Type::Named(name.to_string()))],
            named_params: Vec::new(),
            return_type: Some(Type::String),
            body: Box::new(Stmt::Block {
                statements: vec![Box::new(Stmt::Return {
                    value: Some(Box::new(derived_show_expr(name, fields))),
                })],
            }),
        }],
    }
}

/// Escape characters that are special in LK template strings.
/// Template strings use `{...}` for interpolation, so `{` and `}` must be
/// escaped. Backslashes are also escaped to avoid `\{` ambiguity.
fn escape_template_literal(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for ch in s.chars() {
        match ch {
            '\\' => out.push_str("\\\\"),
            '{' => out.push_str("\\{"),
            '}' => out.push_str("\\}"),
            _ => out.push(ch),
        }
    }
    out
}

fn derived_show_expr(name: &str, fields: &[(String, Option<Type>)]) -> Expr {
    let mut parts = Vec::new();
    if fields.is_empty() {
        parts.push(TemplateStringPart::Literal(format!(
            "{} {{}}",
            escape_template_literal(name)
        )));
        return Expr::TemplateString(parts);
    }

    parts.push(TemplateStringPart::Literal(format!(
        "{} {{ ",
        escape_template_literal(name)
    )));
    for (index, (field, _)) in fields.iter().enumerate() {
        if index > 0 {
            parts.push(TemplateStringPart::Literal(", ".to_string()));
        }
        parts.push(TemplateStringPart::Literal(format!(
            "{}: ",
            escape_template_literal(field)
        )));
        parts.push(TemplateStringPart::Expr(Box::new(field_access_expr("self", field))));
    }
    parts.push(TemplateStringPart::Literal(" }".to_string()));
    Expr::TemplateString(parts)
}

fn field_access_expr(base: &str, field: &str) -> Expr {
    Expr::Access(
        Box::new(Expr::Var(base.to_string())),
        Box::new(Expr::Literal(LiteralVal::from_str(field))),
    )
}

fn expand_external_derive(
    derive: &str,
    item: &Stmt,
    options: &ProcMacroOptions,
    fallback_span: Option<&Span>,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    let Some(config) = options.providers.derive_provider(derive) else {
        return Err(error_from_span(
            fallback_span,
            &format!("No procedural derive provider registered for `{derive}`"),
        ));
    };
    let request = derive_request(derive, item, options, fallback_span)?;
    let response = run_proc_macro_process(&request, config)
        .map_err(|err| error_from_span(fallback_span, &format!("Procedural derive `{derive}` failed: {err}")))?;
    reject_error_diagnostics(derive, &response.diagnostics, fallback_span)?;
    options.dependency_recorder.record(&response.dependencies);
    parse_proc_macro_output_items(derive, &response.output_tokens, fallback_span)
}

fn derive_request(
    macro_name: &str,
    item: &Stmt,
    options: &ProcMacroOptions,
    fallback_span: Option<&Span>,
) -> Result<ProcMacroRequest, ParseError> {
    Ok(ProcMacroRequest {
        protocol_version: PROC_MACRO_PROTOCOL_VERSION,
        kind: ProcMacroKind::Derive,
        macro_name: macro_name.to_string(),
        input_tokens: derive_attribute_tokens(macro_name),
        item_tokens: stmt_item_tokens(item, fallback_span, macro_name)?,
        package: options.package.clone(),
        module: options.module.clone(),
        features: options.features.clone(),
    })
}

fn derive_attribute_tokens(name: &str) -> Vec<ProcMacroToken> {
    [
        Token::Id("derive".to_string()),
        Token::LParen,
        Token::Id(name.to_string()),
        Token::RParen,
    ]
    .iter()
    .map(|token| ProcMacroToken::from_token(token, None))
    .collect()
}
