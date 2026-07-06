use super::{AstGeneratedItemOrigin, AstGeneratedMemberOrigin};
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::{
    expr::{Expr, Pattern, TemplateStringPart},
    operator::{BinOp, UnaryOp},
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
                push_generated_statement_origin("stmt param", span.clone(), &mut origins);
                push_generated_reference_origin("binding", param, span.clone(), &mut origins);
            }
            for ty in param_types.iter().flatten() {
                push_generated_statement_origin("stmt param_type", span.clone(), &mut origins);
                collect_generated_type_origins(ty, span.clone(), &mut origins);
            }
            for param in named_params {
                push_generated_statement_origin("stmt named_param", span.clone(), &mut origins);
                push_generated_reference_origin("binding", &param.name, span.clone(), &mut origins);
                if let Some(ty) = &param.type_annotation {
                    push_generated_statement_origin("stmt named_param_type", span.clone(), &mut origins);
                    collect_generated_type_origins(ty, span.clone(), &mut origins);
                }
                if let Some(default) = &param.default {
                    push_generated_statement_origin("stmt param_default", span.clone(), &mut origins);
                    collect_generated_expr_origins(default, span.clone(), &mut origins);
                }
            }
            if let Some(ty) = return_type {
                push_generated_statement_origin("stmt return_type", span.clone(), &mut origins);
                collect_generated_type_origins(ty, span.clone(), &mut origins);
            }
            push_generated_statement_origin("stmt function_body", span.clone(), &mut origins);
            collect_generated_expr_origins_from_stmt(body, span, &mut origins);
            origins
        }
        Stmt::Trait { name, methods } => {
            let mut origins = vec![AstGeneratedMemberOrigin {
                label: format!("trait {name}"),
                span: span.clone(),
            }];
            for (name, ty) in methods {
                push_generated_statement_origin("stmt trait_method", span.clone(), &mut origins);
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("fn {name}"),
                    span: span.clone(),
                });
                push_generated_statement_origin("stmt trait_method_type", span.clone(), &mut origins);
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
            push_generated_statement_origin("stmt impl_trait", span.clone(), &mut origins);
            push_generated_statement_origin("stmt impl_target", span.clone(), &mut origins);
            collect_generated_type_origins(target_type, span.clone(), &mut origins);
            origins.extend(methods.iter().flat_map(|method| {
                let mut origins = vec![AstGeneratedMemberOrigin {
                    label: "stmt impl_method".to_string(),
                    span: span.clone(),
                }];
                origins.extend(generated_member_origins_for_stmt(method, span.clone()));
                origins
            }));
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
                    push_generated_statement_origin("stmt struct_field_type", span.clone(), &mut origins);
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
            push_generated_statement_origin("stmt type_alias_target", span.clone(), &mut origins);
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
        Type::Named(name) => {
            push_generated_statement_origin("type_expr named", span.clone(), origins);
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type_ref {name}"),
                span,
            });
        }
        Type::Generic { name, params } => {
            push_generated_statement_origin("type_expr generic", span.clone(), origins);
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type_ref {name}"),
                span: span.clone(),
            });
            for param in params {
                push_generated_statement_origin("type_expr generic_arg", span.clone(), origins);
                collect_generated_type_origins(param, span.clone(), origins);
            }
        }
        Type::List(inner) => {
            push_generated_statement_origin("type_expr list", span.clone(), origins);
            push_generated_statement_origin("type_expr list_item", span.clone(), origins);
            collect_generated_type_origins(inner, span, origins);
        }
        Type::Set(inner) => {
            push_generated_statement_origin("type_expr set", span.clone(), origins);
            push_generated_statement_origin("type_expr set_item", span.clone(), origins);
            collect_generated_type_origins(inner, span, origins);
        }
        Type::Task(inner) => {
            push_generated_statement_origin("type_expr task", span.clone(), origins);
            push_generated_statement_origin("type_expr task_output", span.clone(), origins);
            collect_generated_type_origins(inner, span, origins);
        }
        Type::Channel(inner) => {
            push_generated_statement_origin("type_expr channel", span.clone(), origins);
            push_generated_statement_origin("type_expr channel_item", span.clone(), origins);
            collect_generated_type_origins(inner, span, origins);
        }
        Type::Optional(inner) => {
            push_generated_statement_origin("type_expr optional", span.clone(), origins);
            push_generated_statement_origin("type_expr optional_inner", span.clone(), origins);
            collect_generated_type_origins(inner, span, origins);
        }
        Type::Boxed(inner) => {
            push_generated_statement_origin("type_expr boxed", span.clone(), origins);
            push_generated_statement_origin("type_expr boxed_inner", span.clone(), origins);
            collect_generated_type_origins(inner, span, origins);
        }
        Type::Map(key, value) => {
            push_generated_statement_origin("type_expr map", span.clone(), origins);
            push_generated_statement_origin("type_expr map_key", span.clone(), origins);
            collect_generated_type_origins(key, span.clone(), origins);
            push_generated_statement_origin("type_expr map_value", span.clone(), origins);
            collect_generated_type_origins(value, span, origins);
        }
        Type::Tuple(items) => {
            push_generated_statement_origin("type_expr tuple", span.clone(), origins);
            for item in items {
                push_generated_statement_origin("type_expr tuple_item", span.clone(), origins);
                collect_generated_type_origins(item, span.clone(), origins);
            }
        }
        Type::Union(items) => {
            push_generated_statement_origin("type_expr union", span.clone(), origins);
            for item in items {
                push_generated_statement_origin("type_expr union_item", span.clone(), origins);
                collect_generated_type_origins(item, span.clone(), origins);
            }
        }
        Type::Function {
            params,
            named_params,
            return_type,
        } => {
            push_generated_statement_origin("type_expr function", span.clone(), origins);
            for param in params {
                push_generated_statement_origin("type_expr function_param", span.clone(), origins);
                collect_generated_type_origins(param, span.clone(), origins);
            }
            for param in named_params {
                push_generated_statement_origin("type_expr function_named_param", span.clone(), origins);
                push_generated_reference_origin("named_param_type", &param.name, span.clone(), origins);
                collect_generated_type_origins(&param.ty, span.clone(), origins);
            }
            push_generated_statement_origin("type_expr function_return", span.clone(), origins);
            collect_generated_type_origins(return_type, span, origins);
        }
        Type::Variable(name) => {
            push_generated_statement_origin("type_expr variable", span.clone(), origins);
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type_var {name}"),
                span,
            });
        }
        Type::Int => push_generated_statement_origin("type_expr int", span, origins),
        Type::Float => push_generated_statement_origin("type_expr float", span, origins),
        Type::String => push_generated_statement_origin("type_expr string", span, origins),
        Type::Bool => push_generated_statement_origin("type_expr bool", span, origins),
        Type::Nil => push_generated_statement_origin("type_expr nil", span, origins),
        Type::Any => push_generated_statement_origin("type_expr any", span, origins),
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
            push_generated_statement_origin("stmt if_condition", span.clone(), origins);
            collect_generated_expr_origins(condition, span.clone(), origins);
            push_generated_statement_origin("stmt if_then", span.clone(), origins);
            collect_generated_expr_origins_from_stmt(then_stmt, span.clone(), origins);
            if let Some(else_stmt) = else_stmt {
                push_generated_statement_origin("stmt if_else", span.clone(), origins);
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
            push_generated_statement_origin("stmt if_let_pattern", span.clone(), origins);
            collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            push_generated_statement_origin("stmt if_let_value", span.clone(), origins);
            collect_generated_expr_origins(value, span.clone(), origins);
            push_generated_statement_origin("stmt if_let_then", span.clone(), origins);
            collect_generated_expr_origins_from_stmt(then_stmt, span.clone(), origins);
            if let Some(else_stmt) = else_stmt {
                push_generated_statement_origin("stmt if_let_else", span.clone(), origins);
                collect_generated_expr_origins_from_stmt(else_stmt, span, origins);
            }
        }
        Stmt::While { condition, body } => {
            push_generated_statement_origin("stmt while", span.clone(), origins);
            push_generated_statement_origin("stmt while_condition", span.clone(), origins);
            collect_generated_expr_origins(condition, span.clone(), origins);
            push_generated_statement_origin("stmt while_body", span.clone(), origins);
            collect_generated_expr_origins_from_stmt(body, span, origins);
        }
        Stmt::WhileLet { pattern, value, body } => {
            push_generated_statement_origin("stmt while let", span.clone(), origins);
            push_generated_statement_origin("stmt while_let_pattern", span.clone(), origins);
            collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            push_generated_statement_origin("stmt while_let_value", span.clone(), origins);
            collect_generated_expr_origins(value, span.clone(), origins);
            push_generated_statement_origin("stmt while_let_body", span.clone(), origins);
            collect_generated_expr_origins_from_stmt(body, span, origins);
        }
        Stmt::For {
            pattern,
            iterable,
            body,
        } => {
            push_generated_statement_origin("stmt for", span.clone(), origins);
            push_generated_statement_origin("stmt for_pattern", span.clone(), origins);
            collect_generated_for_pattern_binding_origins(pattern, span.clone(), origins);
            push_generated_statement_origin("stmt for_iterable", span.clone(), origins);
            collect_generated_expr_origins(iterable, span.clone(), origins);
            push_generated_statement_origin("stmt for_body", span.clone(), origins);
            collect_generated_expr_origins_from_stmt(body, span, origins);
        }
        Stmt::Let {
            pattern,
            type_annotation,
            value,
            is_const,
            ..
        } => {
            push_generated_statement_origin(if *is_const { "stmt const" } else { "stmt let" }, span.clone(), origins);
            push_generated_statement_origin("stmt binding_pattern", span.clone(), origins);
            collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            if let Some(ty) = type_annotation {
                push_generated_statement_origin("stmt type_annotation", span.clone(), origins);
                collect_generated_type_origins(ty, span.clone(), origins);
            }
            push_generated_statement_origin("stmt initializer", span.clone(), origins);
            collect_generated_expr_origins(value, span, origins);
        }
        Stmt::Assign { name, value, .. } => {
            push_generated_statement_origin("stmt assign", span.clone(), origins);
            push_generated_reference_origin("assign_ref", name, span.clone(), origins);
            push_generated_statement_origin("stmt assign_value", span.clone(), origins);
            collect_generated_expr_origins(value, span, origins);
        }
        Stmt::CompoundAssign { name, op, value, .. } => {
            push_generated_statement_origin("stmt compound_assign", span.clone(), origins);
            push_generated_statement_origin(generated_compound_assign_origin_label(op), span.clone(), origins);
            push_generated_reference_origin("compound_assign_ref", name, span.clone(), origins);
            push_generated_statement_origin("stmt compound_assign_value", span.clone(), origins);
            collect_generated_expr_origins(value, span, origins);
        }
        Stmt::Define { name, value } => {
            push_generated_statement_origin("stmt define", span.clone(), origins);
            push_generated_reference_origin("binding", name, span.clone(), origins);
            push_generated_statement_origin("stmt initializer", span.clone(), origins);
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
                push_generated_statement_origin("stmt param", span.clone(), origins);
                push_generated_reference_origin("binding", param, span.clone(), origins);
            }
            for ty in param_types.iter().flatten() {
                push_generated_statement_origin("stmt param_type", span.clone(), origins);
                collect_generated_type_origins(ty, span.clone(), origins);
            }
            for param in named_params {
                push_generated_statement_origin("stmt named_param", span.clone(), origins);
                push_generated_reference_origin("binding", &param.name, span.clone(), origins);
                if let Some(ty) = &param.type_annotation {
                    push_generated_statement_origin("stmt named_param_type", span.clone(), origins);
                    collect_generated_type_origins(ty, span.clone(), origins);
                }
                if let Some(default) = &param.default {
                    push_generated_statement_origin("stmt param_default", span.clone(), origins);
                    collect_generated_expr_origins(default, span.clone(), origins);
                }
            }
            if let Some(ty) = return_type {
                push_generated_statement_origin("stmt return_type", span.clone(), origins);
                collect_generated_type_origins(ty, span.clone(), origins);
            }
            push_generated_statement_origin("stmt function_body", span.clone(), origins);
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
            push_generated_statement_origin("stmt impl_trait", span.clone(), origins);
            push_generated_statement_origin("stmt impl_target", span.clone(), origins);
            collect_generated_type_origins(target_type, span.clone(), origins);
            for method in methods {
                push_generated_statement_origin("stmt impl_method", span.clone(), origins);
                collect_generated_expr_origins_from_stmt(method, span.clone(), origins);
            }
        }
        Stmt::Expr(expr) => {
            push_generated_statement_origin("stmt expr", span.clone(), origins);
            push_generated_statement_origin("stmt expr_value", span.clone(), origins);
            collect_generated_expr_origins(expr, span, origins);
        }
        Stmt::Return { value } => {
            push_generated_statement_origin("stmt return", span.clone(), origins);
            if let Some(value) = value {
                push_generated_statement_origin("stmt return_value", span.clone(), origins);
                collect_generated_expr_origins(value, span, origins);
            }
        }
        Stmt::Block { statements } => {
            push_generated_statement_origin("stmt block", span.clone(), origins);
            for statement in statements {
                push_generated_statement_origin("stmt block_item", span.clone(), origins);
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
                    push_generated_statement_origin("stmt struct_field_type", span.clone(), origins);
                    collect_generated_type_origins(ty, span.clone(), origins);
                }
            }
        }
        Stmt::TypeAlias { name, target } => {
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type {name}"),
                span: span.clone(),
            });
            push_generated_statement_origin("stmt type_alias_target", span.clone(), origins);
            collect_generated_type_origins(target, span, origins);
        }
        Stmt::Trait { name, methods } => {
            origins.push(AstGeneratedMemberOrigin {
                label: format!("trait {name}"),
                span: span.clone(),
            });
            for (method, ty) in methods {
                push_generated_statement_origin("stmt trait_method", span.clone(), origins);
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("fn {method}"),
                    span: span.clone(),
                });
                push_generated_statement_origin("stmt trait_method_type", span.clone(), origins);
                collect_generated_type_origins(ty, span.clone(), origins);
            }
        }
        Stmt::Import(import) => collect_generated_import_origins(import, span, origins),
        Stmt::Break => push_generated_statement_origin("stmt break", span, origins),
        Stmt::Continue => push_generated_statement_origin("stmt continue", span, origins),
        Stmt::Empty => push_generated_statement_origin("stmt empty", span, origins),
    }
}

fn generated_attribute_origins(attributes: &[Attribute], span: Option<Span>) -> Vec<AstGeneratedMemberOrigin> {
    let mut origins = Vec::new();
    for attribute in attributes {
        let Some(Token::Id(name)) = attribute.tokens.first() else {
            continue;
        };
        push_generated_reference_origin("attr", name, span.clone(), &mut origins);
        collect_generated_attribute_argument_origins(attribute, span.clone(), &mut origins);
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

fn collect_generated_attribute_argument_origins(
    attribute: &Attribute,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    for (index, token) in attribute.tokens.iter().enumerate().skip(1) {
        match token {
            Token::Id(name) => {
                let kind = if matches!(attribute.tokens.get(index + 1), Some(Token::Assign)) {
                    "attr_key"
                } else if matches!(attribute.tokens.get(index - 1), Some(Token::Assign)) {
                    "attr_value"
                } else {
                    "attr_arg"
                };
                push_generated_reference_origin(kind, name, span.clone(), origins);
            }
            Token::Str(value) if is_attribute_value_position(attribute, index) => {
                push_generated_reference_origin("attr_value", value, span.clone(), origins);
            }
            Token::Int(value) if is_attribute_value_position(attribute, index) => {
                push_generated_reference_origin("attr_value", &value.to_string(), span.clone(), origins);
            }
            Token::Float(value) if is_attribute_value_position(attribute, index) => {
                push_generated_reference_origin("attr_value", &value.to_string(), span.clone(), origins);
            }
            Token::Bool(value) if is_attribute_value_position(attribute, index) => {
                push_generated_reference_origin("attr_value", &value.to_string(), span.clone(), origins);
            }
            Token::Nil if is_attribute_value_position(attribute, index) => {
                push_generated_reference_origin("attr_value", "nil", span.clone(), origins);
            }
            _ => {}
        }
    }
}

fn is_attribute_value_position(attribute: &Attribute, index: usize) -> bool {
    matches!(
        attribute.tokens.get(index - 1),
        Some(Token::Assign | Token::LParen | Token::Comma)
    )
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
            } else if let (Some(base), Some(field)) = (expr_access_base_label(base), expr_static_dot_field_name(field))
            {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("expr {base}.{field}"),
                    span: span.clone(),
                });
            }
            push_generated_statement_origin("expr access_base", span.clone(), origins);
            collect_generated_expr_origins(base, span.clone(), origins);
            if expr_static_dot_field_name(field).is_none() && expr_static_index_name(field).is_none() {
                push_generated_statement_origin("expr access_member", span.clone(), origins);
                collect_generated_expr_origins(field, span, origins);
            }
        }
        Expr::Bin(left, op, right) => {
            push_generated_statement_origin("expr binary", span.clone(), origins);
            push_generated_statement_origin(generated_binary_origin_label(op), span.clone(), origins);
            push_generated_statement_origin("expr binary_left", span.clone(), origins);
            collect_generated_expr_origins(left, span.clone(), origins);
            push_generated_statement_origin("expr binary_right", span.clone(), origins);
            collect_generated_expr_origins(right, span, origins);
        }
        Expr::And(left, right) => {
            push_generated_statement_origin("expr and", span.clone(), origins);
            push_generated_statement_origin("expr logical_left", span.clone(), origins);
            collect_generated_expr_origins(left, span.clone(), origins);
            push_generated_statement_origin("expr logical_right", span.clone(), origins);
            collect_generated_expr_origins(right, span, origins);
        }
        Expr::Or(left, right) => {
            push_generated_statement_origin("expr or", span.clone(), origins);
            push_generated_statement_origin("expr logical_left", span.clone(), origins);
            collect_generated_expr_origins(left, span.clone(), origins);
            push_generated_statement_origin("expr logical_right", span.clone(), origins);
            collect_generated_expr_origins(right, span, origins);
        }
        Expr::NullishCoalescing(left, right) => {
            push_generated_statement_origin("expr nullish", span.clone(), origins);
            push_generated_statement_origin("expr nullish_left", span.clone(), origins);
            collect_generated_expr_origins(left, span.clone(), origins);
            push_generated_statement_origin("expr nullish_right", span.clone(), origins);
            collect_generated_expr_origins(right, span, origins);
        }
        Expr::Unary(op, inner) => {
            push_generated_statement_origin("expr unary", span.clone(), origins);
            push_generated_statement_origin(generated_unary_origin_label(op), span.clone(), origins);
            push_generated_statement_origin("expr unary_operand", span.clone(), origins);
            collect_generated_expr_origins(inner, span, origins);
        }
        Expr::Paren(inner) => {
            push_generated_statement_origin("expr paren", span.clone(), origins);
            push_generated_statement_origin("expr paren_inner", span.clone(), origins);
            collect_generated_expr_origins(inner, span, origins);
        }
        Expr::Conditional(condition, then_expr, else_expr) => {
            push_generated_statement_origin("expr conditional", span.clone(), origins);
            push_generated_statement_origin("expr conditional_condition", span.clone(), origins);
            collect_generated_expr_origins(condition, span.clone(), origins);
            push_generated_statement_origin("expr conditional_then", span.clone(), origins);
            collect_generated_expr_origins(then_expr, span.clone(), origins);
            push_generated_statement_origin("expr conditional_else", span.clone(), origins);
            collect_generated_expr_origins(else_expr, span, origins);
        }
        Expr::List(items) => {
            push_generated_statement_origin("expr list", span.clone(), origins);
            for item in items {
                push_generated_statement_origin("expr list_item", span.clone(), origins);
                collect_generated_expr_origins(item, span.clone(), origins);
            }
        }
        Expr::Map(pairs) => {
            push_generated_statement_origin("expr map", span.clone(), origins);
            for (key, value) in pairs {
                push_generated_statement_origin("expr map_key_expr", span.clone(), origins);
                if let Some(key) = expr_static_dot_field_name(key) {
                    push_generated_reference_origin("map_key", &key, span.clone(), origins);
                }
                collect_generated_expr_origins(key, span.clone(), origins);
                push_generated_statement_origin("expr map_value", span.clone(), origins);
                collect_generated_expr_origins(value, span.clone(), origins);
            }
        }
        Expr::StructLiteral { name, fields } => {
            push_generated_statement_origin("expr struct_literal", span.clone(), origins);
            push_generated_statement_origin("expr struct_type", span.clone(), origins);
            origins.push(AstGeneratedMemberOrigin {
                label: format!("type_ref {name}"),
                span: span.clone(),
            });
            for (field, value) in fields {
                push_generated_statement_origin("expr struct_field", span.clone(), origins);
                push_generated_reference_origin("struct_field", field, span.clone(), origins);
                push_generated_statement_origin("expr struct_field_value", span.clone(), origins);
                collect_generated_expr_origins(value, span.clone(), origins);
            }
        }
        Expr::Call(name, args) => {
            if collect_lowered_struct_literal_origins(name, args, span.clone(), origins) {
                return;
            }
            push_generated_statement_origin("expr call", span.clone(), origins);
            push_generated_statement_origin("expr call_callee", span.clone(), origins);
            origins.push(AstGeneratedMemberOrigin {
                label: format!("call {name}"),
                span: span.clone(),
            });
            for arg in args {
                push_generated_statement_origin("expr call_arg", span.clone(), origins);
                collect_generated_expr_origins(arg, span.clone(), origins);
            }
        }
        Expr::CallExpr(callee, args) => {
            push_generated_statement_origin("expr call_expr", span.clone(), origins);
            push_generated_statement_origin("expr call", span.clone(), origins);
            push_generated_statement_origin("expr call_callee", span.clone(), origins);
            if let Some(callee_label) = expr_access_base_label(callee) {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("call {callee_label}"),
                    span: span.clone(),
                });
            }
            collect_generated_expr_origins(callee, span.clone(), origins);
            for arg in args {
                push_generated_statement_origin("expr call_arg", span.clone(), origins);
                collect_generated_expr_origins(arg, span.clone(), origins);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            push_generated_statement_origin("expr call", span.clone(), origins);
            push_generated_statement_origin("expr call_named", span.clone(), origins);
            push_generated_statement_origin("expr call_callee", span.clone(), origins);
            if let Some(callee_label) = expr_access_base_label(callee) {
                origins.push(AstGeneratedMemberOrigin {
                    label: format!("call {callee_label}"),
                    span: span.clone(),
                });
            }
            collect_generated_expr_origins(callee, span.clone(), origins);
            for arg in positional {
                push_generated_statement_origin("expr call_arg", span.clone(), origins);
                collect_generated_expr_origins(arg, span.clone(), origins);
            }
            for (name, arg) in named {
                push_generated_reference_origin("named_arg", name, span.clone(), origins);
                push_generated_statement_origin("expr named_arg_value", span.clone(), origins);
                collect_generated_expr_origins(arg, span.clone(), origins);
            }
        }
        Expr::Range {
            start,
            end,
            inclusive,
            step,
        } => {
            push_generated_statement_origin("expr range", span.clone(), origins);
            push_generated_statement_origin(
                if *inclusive {
                    "range inclusive"
                } else {
                    "range exclusive"
                },
                span.clone(),
                origins,
            );
            if let Some(start) = start {
                push_generated_statement_origin("range start", span.clone(), origins);
                collect_generated_expr_origins(start, span.clone(), origins);
            }
            if let Some(end) = end {
                push_generated_statement_origin("range end", span.clone(), origins);
                collect_generated_expr_origins(end, span.clone(), origins);
            }
            if let Some(step) = step {
                push_generated_statement_origin("range step", span.clone(), origins);
                collect_generated_expr_origins(step, span.clone(), origins);
            }
        }
        Expr::TemplateString(parts) => {
            push_generated_statement_origin("expr template_string", span.clone(), origins);
            for part in parts {
                match part {
                    TemplateStringPart::Literal(_) => {
                        push_generated_statement_origin("template_part literal", span.clone(), origins);
                    }
                    TemplateStringPart::Expr(expr) => {
                        push_generated_statement_origin("template_part expr", span.clone(), origins);
                        collect_generated_expr_origins(expr, span.clone(), origins);
                    }
                }
            }
        }
        Expr::Closure { params, body } => {
            push_generated_statement_origin("expr closure", span.clone(), origins);
            for param in params {
                push_generated_statement_origin("expr closure_param", span.clone(), origins);
                push_generated_reference_origin("binding", param, span.clone(), origins);
            }
            push_generated_statement_origin("expr closure_body", span.clone(), origins);
            collect_generated_expr_origins(body, span, origins);
        }
        Expr::Block(statements) => {
            push_generated_statement_origin("expr block", span.clone(), origins);
            for statement in statements {
                push_generated_statement_origin("expr block_stmt", span.clone(), origins);
                collect_generated_expr_origins_from_stmt(statement, span.clone(), origins);
            }
        }
        Expr::Match { value, arms } => {
            push_generated_statement_origin("expr match", span.clone(), origins);
            push_generated_statement_origin("expr match_value", span.clone(), origins);
            collect_generated_expr_origins(value, span.clone(), origins);
            for arm in arms {
                push_generated_statement_origin("match_arm", span.clone(), origins);
                push_generated_statement_origin("expr match_arm_pattern", span.clone(), origins);
                if pattern_guard_expr(&arm.pattern).is_some() {
                    push_generated_statement_origin("expr match_arm_guard", span.clone(), origins);
                }
                collect_generated_pattern_binding_origins(&arm.pattern, span.clone(), origins);
                push_generated_statement_origin("expr match_arm_body", span.clone(), origins);
                collect_generated_expr_origins(&arm.body, span.clone(), origins);
            }
        }
        Expr::Var(name) => {
            push_generated_statement_origin("expr var", span.clone(), origins);
            push_generated_reference_origin("ref", name, span, origins);
        }
        Expr::Literal(literal) => {
            push_generated_statement_origin("expr literal", span.clone(), origins);
            push_generated_statement_origin(generated_literal_origin_label(literal), span, origins);
        }
    }
}

fn pattern_guard_expr(pattern: &Pattern) -> Option<&Expr> {
    match pattern {
        Pattern::Guard { guard, .. } => Some(guard),
        Pattern::Literal(_)
        | Pattern::Variable(_)
        | Pattern::Wildcard
        | Pattern::List { .. }
        | Pattern::Map { .. }
        | Pattern::Or(_)
        | Pattern::Range { .. } => None,
    }
}

fn generated_literal_origin_label(literal: &LiteralVal) -> &'static str {
    match literal {
        LiteralVal::Int(_) => "literal int",
        LiteralVal::Float(_) => "literal float",
        LiteralVal::Bool(_) => "literal bool",
        LiteralVal::ShortStr(_) | LiteralVal::String(_) => "literal string",
        LiteralVal::Nil => "literal nil",
    }
}

fn generated_binary_origin_label(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "binary add",
        BinOp::Sub => "binary sub",
        BinOp::Mul => "binary mul",
        BinOp::Div => "binary div",
        BinOp::Mod => "binary mod",
        BinOp::Eq => "binary eq",
        BinOp::Ne => "binary ne",
        BinOp::Gt => "binary gt",
        BinOp::Lt => "binary lt",
        BinOp::Ge => "binary ge",
        BinOp::Le => "binary le",
        BinOp::In => "binary in",
    }
}

fn generated_compound_assign_origin_label(op: &BinOp) -> &'static str {
    match op {
        BinOp::Add => "compound_assign add",
        BinOp::Sub => "compound_assign sub",
        BinOp::Mul => "compound_assign mul",
        BinOp::Div => "compound_assign div",
        BinOp::Mod => "compound_assign mod",
        BinOp::Eq => "compound_assign eq",
        BinOp::Ne => "compound_assign ne",
        BinOp::Gt => "compound_assign gt",
        BinOp::Lt => "compound_assign lt",
        BinOp::Ge => "compound_assign ge",
        BinOp::Le => "compound_assign le",
        BinOp::In => "compound_assign in",
    }
}

fn generated_unary_origin_label(op: &UnaryOp) -> &'static str {
    match op {
        UnaryOp::Not => "unary not",
    }
}

fn collect_lowered_struct_literal_origins(
    name: &str,
    args: &[Box<Expr>],
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) -> bool {
    if name != "__lk_make_struct" || args.len() != 2 {
        return false;
    }
    let Some(type_name) = expr_literal_string(args[0].as_ref()) else {
        return false;
    };

    push_generated_statement_origin("expr struct_literal", span.clone(), origins);
    push_generated_statement_origin("expr struct_type", span.clone(), origins);
    origins.push(AstGeneratedMemberOrigin {
        label: format!("type_ref {type_name}"),
        span: span.clone(),
    });
    collect_lowered_struct_fields_origins(args[1].as_ref(), span, origins);
    true
}

fn collect_lowered_struct_fields_origins(
    fields: &Expr,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match fields {
        Expr::Map(pairs) => {
            for (field, value) in pairs {
                push_generated_statement_origin("expr struct_field", span.clone(), origins);
                if let Some(field) = expr_literal_string(field) {
                    push_generated_reference_origin("struct_field", &field, span.clone(), origins);
                } else {
                    push_generated_statement_origin("expr struct_field_key", span.clone(), origins);
                    collect_generated_expr_origins(field, span.clone(), origins);
                }
                push_generated_statement_origin("expr struct_field_value", span.clone(), origins);
                collect_generated_expr_origins(value, span.clone(), origins);
            }
        }
        Expr::Call(name, args) if name == "__lk_merge_fields" && args.len() == 2 => {
            push_generated_statement_origin("expr struct_update_base", span.clone(), origins);
            collect_generated_expr_origins(args[0].as_ref(), span.clone(), origins);
            push_generated_statement_origin("expr struct_update_fields", span.clone(), origins);
            collect_lowered_struct_fields_origins(args[1].as_ref(), span, origins);
        }
        other => {
            push_generated_statement_origin("expr struct_fields", span.clone(), origins);
            collect_generated_expr_origins(other, span, origins);
        }
    }
}

fn expr_literal_string(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(value) => value.as_str().map(str::to_string),
        _ => None,
    }
}

fn collect_generated_pattern_binding_origins(
    pattern: &Pattern,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match pattern {
        Pattern::Variable(name) if name != "_" => {
            push_generated_statement_origin("pattern variable", span.clone(), origins);
            push_generated_reference_origin("binding", name, span, origins);
        }
        Pattern::Variable(_) => {}
        Pattern::List { patterns, rest } => {
            push_generated_statement_origin("pattern list", span.clone(), origins);
            for pattern in patterns {
                push_generated_statement_origin("pattern element", span.clone(), origins);
                collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            }
            if let Some(rest) = rest {
                push_generated_statement_origin("pattern rest", span.clone(), origins);
                push_generated_reference_origin("binding", rest, span, origins);
            }
        }
        Pattern::Map { patterns, rest } => {
            push_generated_statement_origin("pattern map", span.clone(), origins);
            for (key, pattern) in patterns {
                push_generated_statement_origin("pattern key", span.clone(), origins);
                push_generated_reference_origin("map_key", key, span.clone(), origins);
                push_generated_statement_origin("pattern value", span.clone(), origins);
                collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            }
            if let Some(rest) = rest {
                push_generated_statement_origin("pattern rest", span.clone(), origins);
                push_generated_reference_origin("binding", rest, span, origins);
            }
        }
        Pattern::Or(patterns) => {
            push_generated_statement_origin("pattern or", span.clone(), origins);
            for pattern in patterns {
                push_generated_statement_origin("pattern alternative", span.clone(), origins);
                collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            }
        }
        Pattern::Guard { pattern, guard } => {
            push_generated_statement_origin("pattern guard", span.clone(), origins);
            push_generated_statement_origin("match guard", span.clone(), origins);
            collect_generated_pattern_binding_origins(pattern, span.clone(), origins);
            push_generated_statement_origin("pattern guard_expr", span.clone(), origins);
            collect_generated_expr_origins(guard, span, origins);
        }
        Pattern::Range { start, end, inclusive } => {
            push_generated_statement_origin("pattern range", span.clone(), origins);
            push_generated_statement_origin(
                if *inclusive {
                    "pattern range_inclusive"
                } else {
                    "pattern range_exclusive"
                },
                span.clone(),
                origins,
            );
            push_generated_statement_origin("pattern range_start", span.clone(), origins);
            collect_generated_expr_origins(start, span.clone(), origins);
            push_generated_statement_origin("pattern range_end", span.clone(), origins);
            collect_generated_expr_origins(end, span, origins);
        }
        Pattern::Literal(literal) => {
            push_generated_statement_origin("pattern literal", span.clone(), origins);
            push_generated_statement_origin(generated_pattern_literal_origin_label(literal), span, origins);
        }
        Pattern::Wildcard => push_generated_statement_origin("pattern wildcard", span, origins),
    }
}

fn generated_pattern_literal_origin_label(literal: &LiteralVal) -> &'static str {
    match literal {
        LiteralVal::Int(_) => "pattern literal_int",
        LiteralVal::Float(_) => "pattern literal_float",
        LiteralVal::Bool(_) => "pattern literal_bool",
        LiteralVal::ShortStr(_) | LiteralVal::String(_) => "pattern literal_string",
        LiteralVal::Nil => "pattern literal_nil",
    }
}

fn collect_generated_for_pattern_binding_origins(
    pattern: &ForPattern,
    span: Option<Span>,
    origins: &mut Vec<AstGeneratedMemberOrigin>,
) {
    match pattern {
        ForPattern::Variable(name) => {
            push_generated_statement_origin("for_pattern variable", span.clone(), origins);
            push_generated_reference_origin("binding", name, span, origins);
        }
        ForPattern::Tuple(patterns) => {
            push_generated_statement_origin("for_pattern tuple", span.clone(), origins);
            for pattern in patterns {
                push_generated_statement_origin("for_pattern element", span.clone(), origins);
                collect_generated_for_pattern_binding_origins(pattern, span.clone(), origins);
            }
        }
        ForPattern::Array { patterns, rest } => {
            push_generated_statement_origin("for_pattern array", span.clone(), origins);
            for pattern in patterns {
                push_generated_statement_origin("for_pattern element", span.clone(), origins);
                collect_generated_for_pattern_binding_origins(pattern, span.clone(), origins);
            }
            if let Some(rest) = rest {
                push_generated_statement_origin("for_pattern rest", span.clone(), origins);
                push_generated_reference_origin("binding", rest, span, origins);
            }
        }
        ForPattern::Object(entries) => {
            push_generated_statement_origin("for_pattern object", span.clone(), origins);
            for (key, pattern) in entries {
                push_generated_statement_origin("for_pattern key", span.clone(), origins);
                push_generated_reference_origin("map_key", key, span.clone(), origins);
                push_generated_statement_origin("for_pattern value", span.clone(), origins);
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
    expr_static_dot_field_name(expr).or_else(|| expr_static_index_name(expr))
}

fn expr_static_dot_field_name(expr: &Expr) -> Option<String> {
    match expr {
        Expr::Literal(value) => value.as_str().map(str::to_string),
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
mod tests;
