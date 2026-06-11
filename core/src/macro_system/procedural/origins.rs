use super::{AstGeneratedItemOrigin, AstGeneratedMemberOrigin};
use crate::{
    expr::{Expr, Pattern, SelectCase, SelectPattern, TemplateStringPart},
    stmt::{Attribute, ForPattern, ImportSource, ImportStmt, Stmt},
    token::{Span, Token},
    val::{LiteralVal, Type},
};

pub(super) fn generated_item_origins(
    generated_items: &[Box<Stmt>],
    input_span: Option<Span>,
) -> Vec<AstGeneratedItemOrigin> {
    generated_items
        .iter()
        .map(|stmt| AstGeneratedItemOrigin {
            label: stmt_label(stmt),
            span: stmt_span(stmt).cloned().or_else(|| input_span.clone()),
            generated_member_origins: generated_member_origins_for_stmt(stmt, input_span.clone()),
        })
        .collect()
}

pub(super) fn builtin_show_generated_member_origins(
    item_label: &str,
    span: Option<Span>,
    fields: &[(String, Option<Type>)],
) -> Vec<AstGeneratedMemberOrigin> {
    if item_label.starts_with("trait ") {
        return vec![AstGeneratedMemberOrigin {
            label: "fn show".to_string(),
            span,
        }];
    }
    if !item_label.starts_with("impl ") {
        return Vec::new();
    }

    let mut origins = vec![AstGeneratedMemberOrigin {
        label: "fn show".to_string(),
        span: span.clone(),
    }];
    origins.extend(fields.iter().map(|(field, _)| AstGeneratedMemberOrigin {
        label: format!("expr self.{field}"),
        span: span.clone(),
    }));
    origins
}

fn generated_member_origins_for_stmt(stmt: &Stmt, span: Option<Span>) -> Vec<AstGeneratedMemberOrigin> {
    match stmt {
        Stmt::Function {
            name,
            params,
            param_types,
            named_params,
            return_type,
            body,
            ..
        } => {
            let mut origins = vec![AstGeneratedMemberOrigin {
                label: format!("fn {name}"),
                span: span.clone(),
            }];
            for param in params {
                push_generated_reference_origin("binding", param, span.clone(), &mut origins);
            }
            for ty in param_types.iter().flatten() {
                collect_generated_type_origins(ty, span.clone(), &mut origins);
            }
            for param in named_params {
                push_generated_reference_origin("binding", &param.name, span.clone(), &mut origins);
                if let Some(ty) = &param.type_annotation {
                    collect_generated_type_origins(ty, span.clone(), &mut origins);
                }
                if let Some(default) = &param.default {
                    collect_generated_expr_origins(default, span.clone(), &mut origins);
                }
            }
            if let Some(ty) = return_type {
                collect_generated_type_origins(ty, span.clone(), &mut origins);
            }
            collect_generated_expr_origins_from_stmt(body, span, &mut origins);
            origins
        }
        Stmt::Trait { name, methods } => {
            let mut origins = vec![AstGeneratedMemberOrigin {
                label: format!("trait {name}"),
                span: span.clone(),
            }];
            for (name, ty) in methods {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("fn {name}"),
                    span: span.clone(),
                });
                collect_generated_type_origins(ty, span.clone(), &mut origins);
            }
            origins
        }
        Stmt::Impl {
            trait_name,
            target_type,
            methods,
        } => {
            let mut origins = vec![AstGeneratedMemberOrigin {
                label: format!("type_ref {trait_name}"),
                span: span.clone(),
            }];
            collect_generated_type_origins(target_type, span.clone(), &mut origins);
            origins.extend(
                methods
                    .iter()
                    .flat_map(|method| generated_member_origins_for_stmt(method, span.clone())),
            );
            origins
        }
        Stmt::Struct { name, fields } => {
            let mut origins = vec![AstGeneratedMemberOrigin {
                label: format!("struct {name}"),
                span: span.clone(),
            }];
            for (field, ty) in fields {
                push_generated_reference_origin("struct_field", field, span.clone(), &mut origins);
                if let Some(ty) = ty {
                    collect_generated_type_origins(ty, span.clone(), &mut origins);
                }
            }
            origins
        }
        Stmt::TypeAlias { name, target } => {
            let mut origins = vec![AstGeneratedMemberOrigin {
                label: format!("type {name}"),
                span: span.clone(),
            }];
            collect_generated_type_origins(target, span, &mut origins);
            origins
        }
        Stmt::Attributed { attributes, item } => {
            let mut origins = generated_attribute_origins(attributes, span.clone());
            origins.extend(generated_member_origins_for_stmt(item, span));
            origins
        }
        Stmt::Import(import) => {
            let mut origins = Vec::new();
            collect_generated_import_origins(import, span, &mut origins);
            origins
        }
        _ => {
            let mut origins = Vec::new();
            collect_generated_expr_origins_from_stmt(stmt, span, &mut origins);
            origins
        }
    }
}

fn collect_generated_type_origins(ty: &Type, span: Option<Span>, origins: &mut Vec<AstGeneratedMemberOrigin>) {
    match ty {
        Type::Named(name) => origins.push(AstGeneratedMemberOrigin {
            label: format!("type_ref {name}"),
            span,
        }),
        Type::Generic { name, params } => {
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type_ref {name}"),
                span: span.clone(),
            });
            for param in params {
                collect_generated_type_origins(param, span.clone(), origins);
            }
        }
        Type::List(inner)
        | Type::Set(inner)
        | Type::Task(inner)
        | Type::Channel(inner)
        | Type::Optional(inner)
        | Type::Boxed(inner) => collect_generated_type_origins(inner, span, origins),
        Type::Map(key, value) => {
            collect_generated_type_origins(key, span.clone(), origins);
            collect_generated_type_origins(value, span, origins);
        }
        Type::Tuple(items) | Type::Union(items) => {
            for item in items {
                collect_generated_type_origins(item, span.clone(), origins);
            }
        }
        Type::Function {
            params,
            named_params,
            return_type,
        } => {
            for param in params {
                collect_generated_type_origins(param, span.clone(), origins);
            }
            for param in named_params {
                push_generated_reference_origin("named_param_type", &param.name, span.clone(), origins);
                collect_generated_type_origins(&param.ty, span.clone(), origins);
            }
            collect_generated_type_origins(return_type, span, origins);
        }
        Type::Variable(name) => origins.push(AstGeneratedMemberOrigin {
            label: format!("type_var {name}"),
            span,
        }),
        Type::Int | Type::Float | Type::String | Type::Bool | Type::Nil | Type::Any => {}
    }
}

fn collect_generated_expr_origins_from_stmt(
    stmt: &Stmt,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match stmt {
        Stmt::Attributed { attributes, item } => {
            origins.extend(generated_attribute_origins(attributes, span.clone()));
            collect_generated_expr_origins_from_stmt(item, span, origins);
        }
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            push_generated_statement_origin("stmt if", span.clone(), origins);
            collect_generated_expr_origins(condition, span.clone(), origins);
            collect_generated_expr_origins_from_stmt(then_stmt, span.clone(), origins);
            if let Some(else_stmt) = else_stmt {
                collect_generated_expr_origins_from_stmt(else_stmt, span, origins);
            }
        }
        Stmt::IfLet {
            pattern,
            value,
            then_stmt,
            else_stmt,
        } => {
            push_generated_statement_origin("stmt if let", span.clone(), origins);
            collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            collect_generated_expr_origins(value, span.clone(), origins);
            collect_generated_expr_origins_from_stmt(then_stmt, span.clone(), origins);
            if let Some(else_stmt) = else_stmt {
                collect_generated_expr_origins_from_stmt(else_stmt, span, origins);
            }
        }
        Stmt::While { condition, body } => {
            push_generated_statement_origin("stmt while", span.clone(), origins);
            collect_generated_expr_origins(condition, span.clone(), origins);
            collect_generated_expr_origins_from_stmt(body, span, origins);
        }
        Stmt::WhileLet { pattern, value, body } => {
            push_generated_statement_origin("stmt while let", span.clone(), origins);
            collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            collect_generated_expr_origins(value, span.clone(), origins);
            collect_generated_expr_origins_from_stmt(body, span, origins);
        }
        Stmt::For {
            pattern,
            iterable,
            body,
        } => {
            push_generated_statement_origin("stmt for", span.clone(), origins);
            collect_generated_for_pattern_binding_origins(pattern, span.clone(), origins);
            collect_generated_expr_origins(iterable, span.clone(), origins);
            collect_generated_expr_origins_from_stmt(body, span, origins);
        }
        Stmt::Let {
            pattern,
            type_annotation,
            value,
            ..
        } => {
            push_generated_statement_origin("stmt let", span.clone(), origins);
            collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            if let Some(ty) = type_annotation {
                collect_generated_type_origins(ty, span.clone(), origins);
            }
            collect_generated_expr_origins(value, span, origins);
        }
        Stmt::Assign { name, value, .. } => {
            push_generated_statement_origin("stmt assign", span.clone(), origins);
            push_generated_reference_origin("assign_ref", name, span.clone(), origins);
            collect_generated_expr_origins(value, span, origins);
        }
        Stmt::CompoundAssign { name, value, .. } => {
            push_generated_statement_origin("stmt compound_assign", span.clone(), origins);
            push_generated_reference_origin("compound_assign_ref", name, span.clone(), origins);
            collect_generated_expr_origins(value, span, origins);
        }
        Stmt::Define { name, value } => {
            push_generated_statement_origin("stmt define", span.clone(), origins);
            push_generated_reference_origin("binding", name, span.clone(), origins);
            collect_generated_expr_origins(value, span, origins);
        }
        Stmt::Function {
            name,
            params,
            param_types,
            named_params,
            return_type,
            body,
            ..
        } => {
            origins.push(AstGeneratedMemberOrigin {
                label: format!("fn {name}"),
                span: span.clone(),
            });
            for param in params {
                push_generated_reference_origin("binding", param, span.clone(), origins);
            }
            for ty in param_types.iter().flatten() {
                collect_generated_type_origins(ty, span.clone(), origins);
            }
            for param in named_params {
                push_generated_reference_origin("binding", &param.name, span.clone(), origins);
                if let Some(ty) = &param.type_annotation {
                    collect_generated_type_origins(ty, span.clone(), origins);
                }
                if let Some(default) = &param.default {
                    collect_generated_expr_origins(default, span.clone(), origins);
                }
            }
            if let Some(ty) = return_type {
                collect_generated_type_origins(ty, span.clone(), origins);
            }
            collect_generated_expr_origins_from_stmt(body, span, origins);
        }
        Stmt::Impl {
            trait_name,
            target_type,
            methods,
        } => {
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type_ref {trait_name}"),
                span: span.clone(),
            });
            collect_generated_type_origins(target_type, span.clone(), origins);
            for method in methods {
                collect_generated_expr_origins_from_stmt(method, span.clone(), origins);
            }
        }
        Stmt::Expr(expr) => {
            push_generated_statement_origin("stmt expr", span.clone(), origins);
            collect_generated_expr_origins(expr, span, origins);
        }
        Stmt::Return { value } => {
            push_generated_statement_origin("stmt return", span.clone(), origins);
            if let Some(value) = value {
                collect_generated_expr_origins(value, span, origins);
            }
        }
        Stmt::Block { statements } => {
            push_generated_statement_origin("stmt block", span.clone(), origins);
            for statement in statements {
                collect_generated_expr_origins_from_stmt(statement, span.clone(), origins);
            }
        }
        Stmt::Struct { name, fields } => {
            origins.push(AstGeneratedMemberOrigin {
                label: format!("struct {name}"),
                span: span.clone(),
            });
            for (field, ty) in fields {
                push_generated_reference_origin("struct_field", field, span.clone(), origins);
                if let Some(ty) = ty {
                    collect_generated_type_origins(ty, span.clone(), origins);
                }
            }
        }
        Stmt::TypeAlias { name, target } => {
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type {name}"),
                span: span.clone(),
            });
            collect_generated_type_origins(target, span, origins);
        }
        Stmt::Trait { name, methods } => {
            origins.push(AstGeneratedMemberOrigin {
                label: format!("trait {name}"),
                span: span.clone(),
            });
            for (method, ty) in methods {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("fn {method}"),
                    span: span.clone(),
                });
                collect_generated_type_origins(ty, span.clone(), origins);
            }
        }
        Stmt::Import(import) => collect_generated_import_origins(import, span, origins),
        Stmt::Break => push_generated_statement_origin("stmt break", span, origins),
        Stmt::Continue => push_generated_statement_origin("stmt continue", span, origins),
        Stmt::Empty => {}
    }
}

fn generated_attribute_origins(attributes: &[Attribute], span: Option<Span>) -> Vec<AstGeneratedMemberOrigin> {
    let mut origins = Vec::new();
    for attribute in attributes {
        let Some(Token::Id(name)) = attribute.tokens.first() else {
            continue;
        };
        push_generated_reference_origin("attr", name, span.clone(), &mut origins);
        if name == "derive" {
            for token in attribute.tokens.iter().skip(1) {
                if let Token::Id(derive) = token {
                    push_generated_reference_origin("derive", derive, span.clone(), &mut origins);
                }
            }
        }
    }
    origins
}

fn collect_generated_import_origins(
    import: &ImportStmt,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match import {
        ImportStmt::Module { module } => push_generated_reference_origin("import_module", module, span, origins),
        ImportStmt::File { path } => push_generated_reference_origin("import_file", path, span, origins),
        ImportStmt::Items { items, source } => {
            push_generated_import_source_origin(source, span.clone(), origins);
            for item in items {
                push_generated_reference_origin("import_item", &item.name, span.clone(), origins);
                if let Some(alias) = &item.alias {
                    push_generated_reference_origin("import_alias", alias, span.clone(), origins);
                }
            }
        }
        ImportStmt::Namespace { alias, source } => {
            push_generated_import_source_origin(source, span.clone(), origins);
            push_generated_reference_origin("import_namespace", alias, span, origins);
        }
        ImportStmt::ModuleAlias { module, alias } => {
            push_generated_reference_origin("import_module", module, span.clone(), origins);
            push_generated_reference_origin("import_alias", alias, span, origins);
        }
    }
}

fn push_generated_import_source_origin(
    source: &ImportSource,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match source {
        ImportSource::Module(module) => push_generated_reference_origin("import_module", module, span, origins),
        ImportSource::File(path) => push_generated_reference_origin("import_file", path, span, origins),
    }
}

fn collect_generated_expr_origins(expr: &Expr, span: Option<Span>, origins: &mut Vec<AstGeneratedMemberOrigin>) {
    match expr {
        Expr::Access(base, field) | Expr::OptionalAccess(base, field) => {
            push_generated_statement_origin(
                if matches!(expr, Expr::OptionalAccess(_, _)) {
                    "expr optional_access"
                } else {
                    "expr access"
                },
                span.clone(),
                origins,
            );
            if let (Some(base), Some(index)) = (expr_access_base_label(base), expr_static_index_name(field)) {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("index {base}.{index}"),
                    span: span.clone(),
                });
            } else if let (Some(base), Some(field)) = (expr_access_base_label(base), expr_static_field_name(field)) {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("expr {base}.{field}"),
                    span: span.clone(),
                });
            }
            collect_generated_expr_origins(base, span.clone(), origins);
            if expr_static_field_name(field).is_none() {
                collect_generated_expr_origins(field, span, origins);
            }
        }
        Expr::Bin(left, _, right) => {
            push_generated_statement_origin("expr binary", span.clone(), origins);
            collect_generated_expr_origins(left, span.clone(), origins);
            collect_generated_expr_origins(right, span, origins);
        }
        Expr::And(left, right) => {
            push_generated_statement_origin("expr and", span.clone(), origins);
            collect_generated_expr_origins(left, span.clone(), origins);
            collect_generated_expr_origins(right, span, origins);
        }
        Expr::Or(left, right) => {
            push_generated_statement_origin("expr or", span.clone(), origins);
            collect_generated_expr_origins(left, span.clone(), origins);
            collect_generated_expr_origins(right, span, origins);
        }
        Expr::NullishCoalescing(left, right) => {
            push_generated_statement_origin("expr nullish", span.clone(), origins);
            collect_generated_expr_origins(left, span.clone(), origins);
            collect_generated_expr_origins(right, span, origins);
        }
        Expr::Unary(_, inner) => {
            push_generated_statement_origin("expr unary", span.clone(), origins);
            collect_generated_expr_origins(inner, span, origins);
        }
        Expr::Paren(inner) => {
            push_generated_statement_origin("expr paren", span.clone(), origins);
            collect_generated_expr_origins(inner, span, origins);
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            push_generated_statement_origin("expr conditional", span.clone(), origins);
            collect_generated_expr_origins(condition, span.clone(), origins);
            collect_generated_expr_origins(then_expr, span.clone(), origins);
            collect_generated_expr_origins(else_expr, span, origins);
        }
        Expr::List(items) => {
            push_generated_statement_origin("expr list", span.clone(), origins);
            for item in items {
                collect_generated_expr_origins(item, span.clone(), origins);
            }
        }
        Expr::Map(pairs) => {
            push_generated_statement_origin("expr map", span.clone(), origins);
            for (key, value) in pairs {
                if let Some(key) = expr_static_field_name(key) {
                    push_generated_reference_origin("map_key", &key, span.clone(), origins);
                }
                collect_generated_expr_origins(key, span.clone(), origins);
                collect_generated_expr_origins(value, span.clone(), origins);
            }
        }
        Expr::StructLiteral { name, fields } => {
            push_generated_statement_origin("expr struct_literal", span.clone(), origins);
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type_ref {name}"),
                span: span.clone(),
            });
            for (field, value) in fields {
                push_generated_reference_origin("struct_field", field, span.clone(), origins);
                collect_generated_expr_origins(value, span.clone(), origins);
            }
        }
        Expr::Call(name, args) => {
            push_generated_statement_origin("expr call", span.clone(), origins);
            origins.push(AstGeneratedMemberOrigin {
                label: format!("call {name}"),
                span: span.clone(),
            });
            for arg in args {
                collect_generated_expr_origins(arg, span.clone(), origins);
            }
        }
        Expr::CallExpr(callee, args) => {
            push_generated_statement_origin("expr call", span.clone(), origins);
            if let Some(callee_label) = expr_access_base_label(callee) {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("call {callee_label}"),
                    span: span.clone(),
                });
            }
            collect_generated_expr_origins(callee, span.clone(), origins);
            for arg in args {
                collect_generated_expr_origins(arg, span.clone(), origins);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            push_generated_statement_origin("expr call_named", span.clone(), origins);
            if let Some(callee_label) = expr_access_base_label(callee) {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("call {callee_label}"),
                    span: span.clone(),
                });
            }
            collect_generated_expr_origins(callee, span.clone(), origins);
            for arg in positional {
                collect_generated_expr_origins(arg, span.clone(), origins);
            }
            for (name, arg) in named {
                push_generated_reference_origin("named_arg", name, span.clone(), origins);
                collect_generated_expr_origins(arg, span.clone(), origins);
            }
        }
        Expr::Range { start, end, step, .. } => {
            push_generated_statement_origin("expr range", span.clone(), origins);
            for expr in [start, end, step].into_iter().flatten() {
                collect_generated_expr_origins(expr, span.clone(), origins);
            }
        }
        Expr::Select { cases, default_case } => {
            push_generated_statement_origin("expr select", span.clone(), origins);
            for case in cases {
                collect_generated_expr_origins_from_select_case(case, span.clone(), origins);
            }
            if let Some(default_case) = default_case {
                push_generated_statement_origin("select default", span.clone(), origins);
                collect_generated_expr_origins(default_case, span, origins);
            }
        }
        Expr::TemplateString(parts) => {
            push_generated_statement_origin("expr template_string", span.clone(), origins);
            for part in parts {
                if let TemplateStringPart::Expr(expr) = part {
                    collect_generated_expr_origins(expr, span.clone(), origins);
                }
            }
        }
        Expr::Closure { params, body } => {
            push_generated_statement_origin("expr closure", span.clone(), origins);
            for param in params {
                push_generated_reference_origin("binding", param, span.clone(), origins);
            }
            collect_generated_expr_origins(body, span, origins);
        }
        Expr::Block(statements) => {
            push_generated_statement_origin("expr block", span.clone(), origins);
            for statement in statements {
                collect_generated_expr_origins_from_stmt(statement, span.clone(), origins);
            }
        }
        Expr::Match { value, arms } => {
            push_generated_statement_origin("expr match", span.clone(), origins);
            collect_generated_expr_origins(value, span.clone(), origins);
            for arm in arms {
                push_generated_statement_origin("match_arm", span.clone(), origins);
                collect_generated_pattern_binding_origins(&arm.pattern, span.clone(), origins);
                collect_generated_expr_origins(&arm.body, span.clone(), origins);
            }
        }
        Expr::Var(name) => push_generated_reference_origin("ref", name, span, origins),
        Expr::Literal(_) => push_generated_statement_origin("expr literal", span, origins),
    }
}

fn collect_generated_expr_origins_from_select_case(
    case: &SelectCase,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match &case.pattern {
        SelectPattern::Recv { binding, channel } => {
            push_generated_statement_origin("select recv", span.clone(), origins);
            if let Some(binding) = binding {
                push_generated_reference_origin("binding", binding, span.clone(), origins);
            }
            collect_generated_expr_origins(channel, span.clone(), origins);
        }
        SelectPattern::Send { channel, value } => {
            push_generated_statement_origin("select send", span.clone(), origins);
            collect_generated_expr_origins(channel, span.clone(), origins);
            collect_generated_expr_origins(value, span.clone(), origins);
        }
    }
    if let Some(guard) = &case.guard {
        collect_generated_expr_origins(guard, span.clone(), origins);
    }
    collect_generated_expr_origins(&case.body, span, origins);
}

fn collect_generated_pattern_binding_origins(
    pattern: &Pattern,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match pattern {
        Pattern::Variable(name) if name != "_" => push_generated_reference_origin("binding", name, span, origins),
        Pattern::Variable(_) => {}
        Pattern::List { patterns, rest } => {
            push_generated_statement_origin("pattern list", span.clone(), origins);
            for pattern in patterns {
                collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            }
            if let Some(rest) = rest {
                push_generated_reference_origin("binding", rest, span, origins);
            }
        }
        Pattern::Map { patterns, rest } => {
            push_generated_statement_origin("pattern map", span.clone(), origins);
            for (key, pattern) in patterns {
                push_generated_reference_origin("map_key", key, span.clone(), origins);
                collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            }
            if let Some(rest) = rest {
                push_generated_reference_origin("binding", rest, span, origins);
            }
        }
        Pattern::Or(patterns) => {
            push_generated_statement_origin("pattern or", span.clone(), origins);
            for pattern in patterns {
                collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            }
        }
        Pattern::Guard { pattern, guard } => {
            push_generated_statement_origin("pattern guard", span.clone(), origins);
            collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            collect_generated_expr_origins(guard, span, origins);
        }
        Pattern::Range { start, end, .. } => {
            push_generated_statement_origin("pattern range", span.clone(), origins);
            collect_generated_expr_origins(start, span.clone(), origins);
            collect_generated_expr_origins(end, span, origins);
        }
        Pattern::Literal(_) => push_generated_statement_origin("pattern literal", span, origins),
        Pattern::Wildcard => push_generated_statement_origin("pattern wildcard", span, origins),
    }
}

fn collect_generated_for_pattern_binding_origins(
    pattern: &ForPattern,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match pattern {
        ForPattern::Variable(name) => push_generated_reference_origin("binding", name, span, origins),
        ForPattern::Tuple(patterns) => {
            push_generated_statement_origin("for_pattern tuple", span.clone(), origins);
            for pattern in patterns {
                collect_generated_for_pattern_binding_origins(pattern, span.clone(), origins);
            }
        }
        ForPattern::Array { patterns, rest } => {
            push_generated_statement_origin("for_pattern array", span.clone(), origins);
            for pattern in patterns {
                collect_generated_for_pattern_binding_origins(pattern, span.clone(), origins);
            }
            if let Some(rest) = rest {
                push_generated_reference_origin("binding", rest, span, origins);
            }
        }
        ForPattern::Object(entries) => {
            push_generated_statement_origin("for_pattern object", span.clone(), origins);
            for (key, pattern) in entries {
                push_generated_reference_origin("map_key", key, span.clone(), origins);
                collect_generated_for_pattern_binding_origins(pattern, span.clone(), origins);
            }
        }
        ForPattern::Ignore => push_generated_statement_origin("for_pattern ignore", span, origins),
    }
}

fn expr_access_base_label(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Var(name) => Some(name.clone()),
        Expr::Access(base, field) | Expr::OptionalAccess(base, field) => Some(format!(
            "{}.{}",
            expr_access_base_label(base)?,
            expr_static_member_segment(field)?
        )),
        _ => None,
    }
}

fn expr_static_member_segment(expr: &Expr) -> Option<String> {
    expr_static_field_name(expr).or_else(|| expr_static_index_name(expr))
}

fn expr_static_field_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(value) => value.as_str().map(str::to_string),
        Expr::Var(name) => Some(name.clone()),
        _ => None,
    }
}

fn expr_static_index_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(LiteralVal::Int(value)) => Some(value.to_string()),
        _ => None,
    }
}

fn push_generated_statement_origin(label: &str, span: Option<Span>, origins: &mut Vec<AstGeneratedMemberOrigin>) {
    origins.push(AstGeneratedMemberOrigin {
        label: label.to_string(),
        span,
    });
}

fn push_generated_reference_origin(
    kind: &str,
    name: &str,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    origins.push(AstGeneratedMemberOrigin {
        label: format!("{kind} {name}"),
        span,
    });
}

pub(super) fn stmt_label(stmt: &Stmt) -> String {
    match stmt {
        Stmt::Function { name, .. } => format!("fn {name}"),
        Stmt::Struct { name, .. } => format!("struct {name}"),
        Stmt::Trait { name, .. } => format!("trait {name}"),
        Stmt::Impl {
            trait_name,
            target_type,
            ..
        } => format!("impl {trait_name} for {}", target_type.display()),
        Stmt::TypeAlias { name, .. } => format!("type {name}"),
        Stmt::Attributed { item, .. } => stmt_label(item),
        Stmt::Block { .. } => "block".to_string(),
        _ => "statement".to_string(),
    }
}

pub(super) fn stmt_span(stmt: &Stmt) -> Option<&Span> {
    match stmt {
        Stmt::Attributed { attributes, item } => attributes
            .first()
            .and_then(|attr| attr.span.as_ref())
            .or_else(|| stmt_span(item)),
        Stmt::Let { span, .. } | Stmt::Assign { span, .. } | Stmt::CompoundAssign { span, .. } => span.as_ref(),
        Stmt::Block { statements } => statements.iter().find_map(|stmt| stmt_span(stmt)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_type_variable_origin_is_recorded() {
        let span = Some(Span::new(
            crate::token::Position::new(1, 1, 0),
            crate::token::Position::new(1, 3, 2),
        ));
        let mut origins = Vec::new();

        collect_generated_type_origins(&Type::Variable("T".to_string()), span.clone(), &mut origins);

        assert_eq!(
            origins,
            vec![AstGeneratedMemberOrigin {
                label: "type_var T".to_string(),
                span,
            }]
        );
    }
}
