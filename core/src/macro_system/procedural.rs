use crate::{
    expr::{Expr, TemplateStringPart},
    macro_system::token_lexeme,
    stmt::{Attribute, Program, Stmt},
    token::{ParseError, Span, Token},
    val::{LiteralVal, Type},
};
use serde::{Deserialize, Serialize};

pub const PROC_MACRO_PROTOCOL_VERSION: u32 = 1;
const BUILTIN_SHOW_TRAIT: &str = "__LKShow";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcMacroKind {
    FunctionLike,
    Derive,
    Attribute,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroSpan {
    pub start_line: u32,
    pub start_column: u32,
    pub start_offset: usize,
    pub end_line: u32,
    pub end_column: u32,
    pub end_offset: usize,
}

impl From<&Span> for ProcMacroSpan {
    fn from(span: &Span) -> Self {
        Self {
            start_line: span.start.line,
            start_column: span.start.column,
            start_offset: span.start.offset,
            end_line: span.end.line,
            end_column: span.end.column,
            end_offset: span.end.offset,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroToken {
    pub kind: String,
    pub lexeme: String,
    pub span: Option<ProcMacroSpan>,
}

impl ProcMacroToken {
    pub fn from_token(token: &Token, span: Option<&Span>) -> Self {
        Self {
            kind: token_kind(token),
            lexeme: token_lexeme(token),
            span: span.map(ProcMacroSpan::from),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroRequest {
    pub protocol_version: u32,
    pub kind: ProcMacroKind,
    pub macro_name: String,
    pub input_tokens: Vec<ProcMacroToken>,
    pub item_tokens: Vec<ProcMacroToken>,
    pub package: Option<String>,
    pub module: Option<String>,
    pub features: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroResponse {
    pub protocol_version: u32,
    pub output_tokens: Vec<ProcMacroToken>,
    pub diagnostics: Vec<ProcMacroDiagnostic>,
    pub dependencies: Vec<ProcMacroDependency>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroDiagnostic {
    pub level: ProcMacroDiagnosticLevel,
    pub message: String,
    pub span: Option<ProcMacroSpan>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ProcMacroDiagnosticLevel {
    Error,
    Warning,
    Note,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcMacroDependency {
    pub path: String,
    pub digest: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ProcMacroOptions {
    pub package: Option<String>,
    pub module: Option<String>,
    pub features: Vec<String>,
}

#[derive(Debug, Default)]
struct AstMacroState {
    show_trait_available: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum DeriveMacro {
    Debug,
    Show,
}

pub fn expand_ast_macros(program: Program, options: ProcMacroOptions) -> Result<Program, ParseError> {
    let mut state = AstMacroState::default();
    let statements = expand_stmt_vec(program.statements, &options, &mut state)?;
    Program::new(statements).map_err(|err| ParseError::new(err.to_string()))
}

fn expand_stmt_vec(
    statements: Vec<Box<Stmt>>,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    let mut expanded = Vec::with_capacity(statements.len());
    for stmt in statements {
        expanded.extend(expand_stmt(*stmt, options, state)?);
    }
    Ok(expanded)
}

fn expand_stmt(
    stmt: Stmt,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    match stmt {
        Stmt::Attributed { attributes, item } => expand_attributed_stmt(attributes, *item, options, state),
        stmt => Ok(vec![Box::new(expand_stmt_no_split(stmt, options, state)?)]),
    }
}

fn expand_stmt_no_split(stmt: Stmt, options: &ProcMacroOptions, state: &mut AstMacroState) -> Result<Stmt, ParseError> {
    match stmt {
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => Ok(Stmt::If {
            condition,
            then_stmt: expand_stmt_box_to_single(then_stmt, options, state)?,
            else_stmt: else_stmt
                .map(|stmt| expand_stmt_box_to_single(stmt, options, state))
                .transpose()?,
        }),
        Stmt::IfLet {
            pattern,
            value,
            then_stmt,
            else_stmt,
        } => Ok(Stmt::IfLet {
            pattern,
            value,
            then_stmt: expand_stmt_box_to_single(then_stmt, options, state)?,
            else_stmt: else_stmt
                .map(|stmt| expand_stmt_box_to_single(stmt, options, state))
                .transpose()?,
        }),
        Stmt::While { condition, body } => Ok(Stmt::While {
            condition,
            body: expand_stmt_box_to_single(body, options, state)?,
        }),
        Stmt::WhileLet { pattern, value, body } => Ok(Stmt::WhileLet {
            pattern,
            value,
            body: expand_stmt_box_to_single(body, options, state)?,
        }),
        Stmt::For {
            pattern,
            iterable,
            body,
        } => Ok(Stmt::For {
            pattern,
            iterable,
            body: expand_stmt_box_to_single(body, options, state)?,
        }),
        Stmt::Function {
            name,
            params,
            param_types,
            named_params,
            return_type,
            body,
        } => Ok(Stmt::Function {
            name,
            params,
            param_types,
            named_params,
            return_type,
            body: expand_stmt_box_to_single(body, options, state)?,
        }),
        Stmt::Impl {
            trait_name,
            target_type,
            methods,
        } => {
            let mut expanded_methods = Vec::with_capacity(methods.len());
            for method in methods {
                expanded_methods.push(expand_stmt_no_split(method, options, state)?);
            }
            Ok(Stmt::Impl {
                trait_name,
                target_type,
                methods: expanded_methods,
            })
        }
        Stmt::Block { statements } => Ok(Stmt::Block {
            statements: expand_stmt_vec(statements, options, state)?,
        }),
        other => {
            if matches!(other, Stmt::Trait { ref name, .. } if name == BUILTIN_SHOW_TRAIT) {
                state.show_trait_available = true;
            }
            Ok(other)
        }
    }
}

fn expand_stmt_box_to_single(
    stmt: Box<Stmt>,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Box<Stmt>, ParseError> {
    let expanded = expand_stmt(*stmt, options, state)?;
    if expanded.len() == 1 {
        return Ok(expanded.into_iter().next().expect("single expanded statement"));
    }
    Ok(Box::new(Stmt::Block { statements: expanded }))
}

fn expand_attributed_stmt(
    attributes: Vec<Attribute>,
    item: Stmt,
    options: &ProcMacroOptions,
    state: &mut AstMacroState,
) -> Result<Vec<Box<Stmt>>, ParseError> {
    let mut preserved_attrs = Vec::new();
    let mut derives = Vec::new();
    for attr in attributes {
        if let Some(parsed) = parse_derive_attribute(&attr)? {
            for derive in parsed {
                if !derives.contains(&derive) {
                    derives.push(derive);
                }
            }
        } else {
            preserved_attrs.push(attr);
        }
    }

    let expanded_item = expand_stmt_no_split(item, options, state)?;
    if derives.is_empty() {
        return Ok(vec![Box::new(apply_preserved_attributes(
            preserved_attrs,
            expanded_item,
        ))]);
    }

    let Stmt::Struct { name, fields } = expanded_item else {
        return Err(error_from_attrs(
            &preserved_attrs,
            "derive macros currently support structs only",
        ));
    };

    let _requests: Vec<ProcMacroRequest> = derives
        .iter()
        .map(|derive| derive_request(*derive, &name, &fields, options))
        .collect();

    let mut output = Vec::new();
    if !state.show_trait_available {
        output.push(Box::new(builtin_show_trait()));
        state.show_trait_available = true;
    }

    let item = Stmt::Struct {
        name: name.clone(),
        fields: fields.clone(),
    };
    output.push(Box::new(apply_preserved_attributes(preserved_attrs, item)));

    output.push(Box::new(derive_show_impl(&name, &fields)));
    Ok(output)
}

fn parse_derive_attribute(attr: &Attribute) -> Result<Option<Vec<DeriveMacro>>, ParseError> {
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
                derives.push(parse_derive_name(name, attr)?);
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

fn parse_derive_name(name: &str, attr: &Attribute) -> Result<DeriveMacro, ParseError> {
    match name {
        "Debug" => Ok(DeriveMacro::Debug),
        "Show" => Ok(DeriveMacro::Show),
        _ => Err(error_at_attr(attr, &format!("Unsupported derive macro `{name}`"))),
    }
}

fn apply_preserved_attributes(attributes: Vec<Attribute>, item: Stmt) -> Stmt {
    if attributes.is_empty() {
        item
    } else {
        Stmt::Attributed {
            attributes,
            item: Box::new(item),
        }
    }
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

fn derived_show_expr(name: &str, fields: &[(String, Option<Type>)]) -> Expr {
    let mut parts = Vec::new();
    if fields.is_empty() {
        parts.push(TemplateStringPart::Literal(format!("{name} {{}}")));
        return Expr::TemplateString(parts);
    }

    parts.push(TemplateStringPart::Literal(format!("{name} {{ ")));
    for (index, (field, _)) in fields.iter().enumerate() {
        if index > 0 {
            parts.push(TemplateStringPart::Literal(", ".to_string()));
        }
        parts.push(TemplateStringPart::Literal(format!("{field}: ")));
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

fn derive_request(
    derive: DeriveMacro,
    name: &str,
    fields: &[(String, Option<Type>)],
    options: &ProcMacroOptions,
) -> ProcMacroRequest {
    let macro_name = match derive {
        DeriveMacro::Debug => "Debug",
        DeriveMacro::Show => "Show",
    };
    ProcMacroRequest {
        protocol_version: PROC_MACRO_PROTOCOL_VERSION,
        kind: ProcMacroKind::Derive,
        macro_name: macro_name.to_string(),
        input_tokens: derive_attribute_tokens(macro_name),
        item_tokens: struct_item_tokens(name, fields),
        package: options.package.clone(),
        module: options.module.clone(),
        features: options.features.clone(),
    }
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

fn struct_item_tokens(name: &str, fields: &[(String, Option<Type>)]) -> Vec<ProcMacroToken> {
    let mut tokens = vec![Token::Struct, Token::Id(name.to_string()), Token::LBrace];
    for (index, (field, ty)) in fields.iter().enumerate() {
        if index > 0 {
            tokens.push(Token::Comma);
        }
        tokens.push(Token::Id(field.clone()));
        if let Some(ty) = ty {
            tokens.push(Token::Colon);
            tokens.push(Token::Id(ty.display()));
        }
    }
    tokens.push(Token::RBrace);
    tokens
        .iter()
        .map(|token| ProcMacroToken::from_token(token, None))
        .collect()
}

fn token_kind(token: &Token) -> String {
    let debug = format!("{token:?}");
    debug.split(['(', ' ']).next().unwrap_or(debug.as_str()).to_string()
}

fn error_at_attr(attr: &Attribute, message: &str) -> ParseError {
    if let Some(span) = &attr.span {
        ParseError::with_span(message.to_string(), span.clone())
    } else {
        ParseError::new(message.to_string())
    }
}

fn error_from_attrs(attrs: &[Attribute], message: &str) -> ParseError {
    if let Some(attr) = attrs.first()
        && let Some(span) = &attr.span
    {
        return ParseError::with_span(message.to_string(), span.clone());
    }
    ParseError::new(message.to_string())
}

#[cfg(test)]
mod tests {
    use crate::{syntax::ParseOptions, syntax::parse_program_source, val::RuntimeVal, vm::execute_source};

    #[test]
    fn derive_debug_generates_runtime_show_for_template_display() {
        let result = execute_source(
            r#"
            #[derive(Debug)]
            struct User {
                id: Int,
                name: String,
            }

            let user = User { id: 7, name: "Ada" };
            return "${user}" == "User { id: 7, name: Ada }";
            "#,
        )
        .expect("execute derived debug");

        assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
    }

    #[test]
    fn derive_show_preserves_non_macro_attributes() {
        let result = execute_source(
            r#"
            #[repr("lk")]
            #[derive(Show)]
            struct Empty {}

            let value = Empty {};
            return "${value}" == "Empty {}";
            "#,
        )
        .expect("execute derived show with preserved attr");

        assert_eq!(result.returns, vec![RuntimeVal::Bool(true)]);
    }

    #[test]
    fn unsupported_derive_reports_parse_error() {
        let err = parse_program_source(
            r#"
            #[derive(Clone)]
            struct User { id: Int }
            "#,
            ParseOptions::default(),
        )
        .expect_err("unsupported derive should fail");

        assert!(err.to_string().contains("Unsupported derive macro `Clone`"));
    }
}
