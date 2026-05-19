use std::collections::{HashMap, HashSet};

use crate::{
    expr::{Expr, MatchArm, Pattern, SelectCase, SelectPattern, TemplateStringPart},
    op::BinOp,
    stmt::{ForPattern, Stmt},
    val::{Type, Val},
};

#[derive(Clone, Debug)]
enum ParamFact {
    Unknown,
    Known(Type),
    Conflict,
}

pub(crate) fn infer_direct_function_param_types(
    stmt: &Stmt,
    expr_type_hints: &HashMap<usize, Type>,
) -> HashMap<String, Vec<Option<Type>>> {
    let mut functions = HashMap::new();
    collect_function_arities(stmt, &mut functions);
    if functions.is_empty() {
        return HashMap::new();
    }

    let mut facts: HashMap<String, Vec<ParamFact>> = functions
        .iter()
        .map(|(name, arity)| (name.clone(), vec![ParamFact::Unknown; *arity]))
        .collect();
    visit_stmt_calls(stmt, expr_type_hints, &functions, &mut facts, &mut HashMap::new());

    facts
        .into_iter()
        .map(|(name, slots)| {
            let inferred = slots
                .into_iter()
                .map(|fact| match fact {
                    ParamFact::Known(ty) => Some(ty),
                    ParamFact::Unknown | ParamFact::Conflict => None,
                })
                .collect();
            (name, inferred)
        })
        .collect()
}

pub(crate) fn infer_direct_function_return_types(
    stmt: &Stmt,
    expr_type_hints: &HashMap<usize, Type>,
    param_types: &HashMap<String, Vec<Option<Type>>>,
) -> HashMap<String, Option<Type>> {
    let mut out = HashMap::new();
    collect_function_return_types(stmt, expr_type_hints, param_types, &mut out);
    out
}

fn collect_function_arities(stmt: &Stmt, functions: &mut HashMap<String, usize>) {
    match stmt {
        Stmt::Function { name, params, body, .. } => {
            functions.insert(name.clone(), params.len());
            collect_function_arities(body, functions);
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_function_arities(stmt, functions);
            }
        }
        Stmt::If {
            then_stmt, else_stmt, ..
        } => {
            collect_function_arities(then_stmt, functions);
            if let Some(stmt) = else_stmt {
                collect_function_arities(stmt, functions);
            }
        }
        Stmt::IfLet {
            then_stmt, else_stmt, ..
        } => {
            collect_function_arities(then_stmt, functions);
            if let Some(stmt) = else_stmt {
                collect_function_arities(stmt, functions);
            }
        }
        Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
            collect_function_arities(body, functions);
        }
        Stmt::Impl { .. } => {}
        Stmt::Import(_)
        | Stmt::Let { .. }
        | Stmt::Assign { .. }
        | Stmt::CompoundAssign { .. }
        | Stmt::Define { .. }
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Return { .. }
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. }
        | Stmt::Expr(_)
        | Stmt::Empty => {}
    }
}

fn collect_function_return_types(
    stmt: &Stmt,
    hints: &HashMap<usize, Type>,
    param_types: &HashMap<String, Vec<Option<Type>>>,
    out: &mut HashMap<String, Option<Type>>,
) {
    match stmt {
        Stmt::Function {
            name,
            params,
            param_types: declared_param_types,
            return_type,
            body,
            ..
        } => {
            if let Some(ty) = return_type.as_ref().and_then(normalize_type) {
                out.insert(name.clone(), Some(ty));
                collect_function_return_types(body, hints, param_types, out);
                return;
            }
            let mut env = HashMap::new();
            for (param, ty) in params.iter().zip(declared_param_types.iter()) {
                if let Some(ty) = ty.as_ref().and_then(normalize_type) {
                    env.insert(param.clone(), ty);
                }
            }
            if let Some(types) = param_types.get(name) {
                for (param, ty) in params.iter().zip(types.iter()) {
                    if let Some(ty) = ty {
                        env.insert(param.clone(), ty.clone());
                    }
                }
            }
            let mut fact = ParamFact::Unknown;
            infer_stmt_return(body, hints, &mut env, &mut fact);
            out.insert(
                name.clone(),
                match fact {
                    ParamFact::Known(ty) => Some(ty),
                    ParamFact::Unknown | ParamFact::Conflict => None,
                },
            );
            collect_function_return_types(body, hints, param_types, out);
        }
        Stmt::Block { statements } => {
            for stmt in statements {
                collect_function_return_types(stmt, hints, param_types, out);
            }
        }
        Stmt::If {
            then_stmt, else_stmt, ..
        } => {
            collect_function_return_types(then_stmt, hints, param_types, out);
            if let Some(stmt) = else_stmt {
                collect_function_return_types(stmt, hints, param_types, out);
            }
        }
        Stmt::IfLet {
            then_stmt, else_stmt, ..
        } => {
            collect_function_return_types(then_stmt, hints, param_types, out);
            if let Some(stmt) = else_stmt {
                collect_function_return_types(stmt, hints, param_types, out);
            }
        }
        Stmt::While { body, .. } | Stmt::WhileLet { body, .. } | Stmt::For { body, .. } => {
            collect_function_return_types(body, hints, param_types, out);
        }
        Stmt::Impl { .. } => {}
        Stmt::Import(_)
        | Stmt::Let { .. }
        | Stmt::Assign { .. }
        | Stmt::CompoundAssign { .. }
        | Stmt::Define { .. }
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Return { .. }
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. }
        | Stmt::Expr(_)
        | Stmt::Empty => {}
    }
}

fn visit_stmt_calls(
    stmt: &Stmt,
    hints: &HashMap<usize, Type>,
    functions: &HashMap<String, usize>,
    facts: &mut HashMap<String, Vec<ParamFact>>,
    env: &mut HashMap<String, Type>,
) {
    match stmt {
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            visit_expr_calls(condition, hints, functions, facts, env);
            visit_stmt_calls(then_stmt, hints, functions, facts, &mut env.clone());
            if let Some(stmt) = else_stmt {
                visit_stmt_calls(stmt, hints, functions, facts, &mut env.clone());
            }
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            visit_expr_calls(value, hints, functions, facts, env);
            visit_stmt_calls(then_stmt, hints, functions, facts, &mut env.clone());
            if let Some(stmt) = else_stmt {
                visit_stmt_calls(stmt, hints, functions, facts, &mut env.clone());
            }
        }
        Stmt::While { condition, body } => {
            visit_expr_calls(condition, hints, functions, facts, env);
            visit_stmt_calls(body, hints, functions, facts, &mut env.clone());
        }
        Stmt::WhileLet { value, body, .. } => {
            visit_expr_calls(value, hints, functions, facts, env);
            visit_stmt_calls(body, hints, functions, facts, &mut env.clone());
        }
        Stmt::For {
            pattern,
            iterable,
            body,
        } => {
            visit_expr_calls(iterable, hints, functions, facts, env);
            let mut body_env = env.clone();
            if matches!(iterable.as_ref(), Expr::Range { .. }) {
                bind_for_pattern_type(pattern, Type::Int, &mut body_env);
            }
            visit_stmt_calls(body, hints, functions, facts, &mut body_env);
        }
        Stmt::Let {
            pattern,
            type_annotation,
            value,
            ..
        } => {
            visit_expr_calls(value, hints, functions, facts, env);
            if let Some(ty) = type_annotation.as_ref().and_then(normalize_type) {
                bind_pattern_type(pattern, ty, env);
            } else if let Some(ty) = useful_type(value, hints, env) {
                bind_pattern_type(pattern, ty, env);
            }
        }
        Stmt::Assign { name, value, .. } | Stmt::Define { name, value } => {
            visit_expr_calls(value, hints, functions, facts, env);
            if let Some(ty) = useful_type(value, hints, env) {
                env.insert(name.clone(), ty);
            } else {
                env.remove(name);
            }
        }
        Stmt::CompoundAssign { name, op, value, .. } => {
            visit_expr_calls(value, hints, functions, facts, env);
            if op.is_arith()
                && !matches!(op, BinOp::Div)
                && env.get(name) == Some(&Type::Int)
                && useful_type(value, hints, env) == Some(Type::Int)
            {
                env.insert(name.clone(), Type::Int);
            } else {
                env.remove(name);
            }
        }
        Stmt::Return { value } => {
            if let Some(value) = value {
                visit_expr_calls(value, hints, functions, facts, env);
            }
        }
        Stmt::Function { .. } => {}
        Stmt::Impl { methods, .. } => {
            for method in methods {
                visit_stmt_calls(method, hints, functions, facts, &mut HashMap::new());
            }
        }
        Stmt::Expr(expr) => visit_expr_calls(expr, hints, functions, facts, env),
        Stmt::Block { statements } => {
            for stmt in statements {
                visit_stmt_calls(stmt, hints, functions, facts, env);
            }
        }
        Stmt::Import(_)
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. }
        | Stmt::Empty => {}
    }
}

fn infer_stmt_return(stmt: &Stmt, hints: &HashMap<usize, Type>, env: &mut HashMap<String, Type>, fact: &mut ParamFact) {
    match stmt {
        Stmt::Block { statements } => {
            for stmt in statements {
                infer_stmt_return(stmt, hints, env, fact);
            }
        }
        Stmt::If {
            condition,
            then_stmt,
            else_stmt,
        } => {
            visit_expr_for_return_facts(condition, hints, env);
            infer_stmt_return(then_stmt, hints, &mut env.clone(), fact);
            if let Some(stmt) = else_stmt {
                infer_stmt_return(stmt, hints, &mut env.clone(), fact);
            }
        }
        Stmt::IfLet {
            value,
            then_stmt,
            else_stmt,
            ..
        } => {
            visit_expr_for_return_facts(value, hints, env);
            infer_stmt_return(then_stmt, hints, &mut env.clone(), fact);
            if let Some(stmt) = else_stmt {
                infer_stmt_return(stmt, hints, &mut env.clone(), fact);
            }
        }
        Stmt::While { condition, body } => {
            visit_expr_for_return_facts(condition, hints, env);
            infer_stmt_return(body, hints, &mut env.clone(), fact);
        }
        Stmt::WhileLet { value, body, .. } => {
            visit_expr_for_return_facts(value, hints, env);
            infer_stmt_return(body, hints, &mut env.clone(), fact);
        }
        Stmt::For {
            pattern,
            iterable,
            body,
        } => {
            visit_expr_for_return_facts(iterable, hints, env);
            let mut body_env = env.clone();
            if matches!(iterable.as_ref(), Expr::Range { .. }) {
                bind_for_pattern_type(pattern, Type::Int, &mut body_env);
            }
            infer_stmt_return(body, hints, &mut body_env, fact);
        }
        Stmt::Let {
            pattern,
            type_annotation,
            value,
            ..
        } => {
            visit_expr_for_return_facts(value, hints, env);
            if let Some(ty) = type_annotation.as_ref().and_then(normalize_type) {
                bind_pattern_type(pattern, ty, env);
            } else if let Some(ty) = useful_type(value, hints, env) {
                bind_pattern_type(pattern, ty, env);
            }
        }
        Stmt::Assign { name, value, .. } | Stmt::Define { name, value } => {
            visit_expr_for_return_facts(value, hints, env);
            if let Some(ty) = useful_type(value, hints, env) {
                env.insert(name.clone(), ty);
            } else {
                env.remove(name);
            }
        }
        Stmt::CompoundAssign { name, op, value, .. } => {
            visit_expr_for_return_facts(value, hints, env);
            if op.is_arith()
                && !matches!(op, BinOp::Div)
                && env.get(name) == Some(&Type::Int)
                && useful_type(value, hints, env) == Some(Type::Int)
            {
                env.insert(name.clone(), Type::Int);
            } else {
                env.remove(name);
            }
        }
        Stmt::Return { value } => {
            merge_fact(fact, value.as_ref().and_then(|value| useful_type(value, hints, env)));
        }
        Stmt::Expr(expr) => visit_expr_for_return_facts(expr, hints, env),
        Stmt::Function { .. }
        | Stmt::Impl { .. }
        | Stmt::Import(_)
        | Stmt::Break
        | Stmt::Continue
        | Stmt::Struct { .. }
        | Stmt::TypeAlias { .. }
        | Stmt::Trait { .. }
        | Stmt::Empty => {}
    }
}

fn visit_expr_for_return_facts(expr: &Expr, hints: &HashMap<usize, Type>, env: &HashMap<String, Type>) {
    match expr {
        Expr::Call(_, args) | Expr::CallExpr(_, args) => {
            for arg in args {
                visit_expr_for_return_facts(arg, hints, env);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            visit_expr_for_return_facts(callee, hints, env);
            for arg in positional {
                visit_expr_for_return_facts(arg, hints, env);
            }
            for (_, arg) in named {
                visit_expr_for_return_facts(arg, hints, env);
            }
        }
        Expr::Bin(left, _, right)
        | Expr::And(left, right)
        | Expr::Or(left, right)
        | Expr::NullishCoalescing(left, right)
        | Expr::Access(left, right)
        | Expr::OptionalAccess(left, right) => {
            visit_expr_for_return_facts(left, hints, env);
            visit_expr_for_return_facts(right, hints, env);
        }
        Expr::Unary(_, inner) | Expr::Paren(inner) => visit_expr_for_return_facts(inner, hints, env),
        Expr::Conditional(cond, then_expr, else_expr) => {
            visit_expr_for_return_facts(cond, hints, env);
            visit_expr_for_return_facts(then_expr, hints, env);
            visit_expr_for_return_facts(else_expr, hints, env);
        }
        Expr::List(items) => {
            for item in items {
                visit_expr_for_return_facts(item, hints, env);
            }
        }
        Expr::Map(entries) => {
            for (key, value) in entries {
                visit_expr_for_return_facts(key, hints, env);
                visit_expr_for_return_facts(value, hints, env);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                visit_expr_for_return_facts(value, hints, env);
            }
        }
        Expr::Range { start, end, step, .. } => {
            for expr in start.iter().chain(end.iter()).chain(step.iter()) {
                visit_expr_for_return_facts(expr, hints, env);
            }
        }
        Expr::Select { cases, default_case } => {
            for case in cases {
                match &case.pattern {
                    SelectPattern::Recv { channel, .. } => visit_expr_for_return_facts(channel, hints, env),
                    SelectPattern::Send { channel, value } => {
                        visit_expr_for_return_facts(channel, hints, env);
                        visit_expr_for_return_facts(value, hints, env);
                    }
                }
                if let Some(guard) = &case.guard {
                    visit_expr_for_return_facts(guard, hints, env);
                }
                visit_expr_for_return_facts(&case.body, hints, env);
            }
            if let Some(default_case) = default_case {
                visit_expr_for_return_facts(default_case, hints, env);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                if let TemplateStringPart::Expr(expr) = part {
                    visit_expr_for_return_facts(expr, hints, env);
                }
            }
        }
        Expr::Closure { body, .. } => visit_expr_for_return_facts(body, hints, env),
        Expr::Block(_) => {}
        Expr::Match { value, arms } => {
            visit_expr_for_return_facts(value, hints, env);
            for MatchArm { pattern, body } in arms {
                visit_pattern_return_exprs(pattern, hints, env);
                visit_expr_for_return_facts(body, hints, env);
            }
        }
        Expr::Var(_) | Expr::Val(_) => {
            let _ = useful_type(expr, hints, env);
        }
    }
}

fn visit_pattern_return_exprs(pattern: &Pattern, hints: &HashMap<usize, Type>, env: &HashMap<String, Type>) {
    match pattern {
        Pattern::Guard { pattern, guard } => {
            visit_pattern_return_exprs(pattern, hints, env);
            visit_expr_for_return_facts(guard, hints, env);
        }
        Pattern::Range { start, end, .. } => {
            visit_expr_for_return_facts(start, hints, env);
            visit_expr_for_return_facts(end, hints, env);
        }
        Pattern::List { patterns, .. } | Pattern::Or(patterns) => {
            for pattern in patterns {
                visit_pattern_return_exprs(pattern, hints, env);
            }
        }
        Pattern::Map { patterns, .. } => {
            for (_, pattern) in patterns {
                visit_pattern_return_exprs(pattern, hints, env);
            }
        }
        Pattern::Literal(_) | Pattern::Variable(_) | Pattern::Wildcard => {}
    }
}

fn visit_expr_calls(
    expr: &Expr,
    hints: &HashMap<usize, Type>,
    functions: &HashMap<String, usize>,
    facts: &mut HashMap<String, Vec<ParamFact>>,
    env: &HashMap<String, Type>,
) {
    match expr {
        Expr::Call(name, args) => {
            if functions.get(name).is_some_and(|arity| *arity == args.len())
                && let Some(slots) = facts.get_mut(name)
            {
                for (idx, arg) in args.iter().enumerate() {
                    merge_fact(&mut slots[idx], useful_type(arg, hints, env));
                }
            }
            for arg in args {
                visit_expr_calls(arg, hints, functions, facts, env);
            }
        }
        Expr::CallExpr(callee, args) => {
            if let Expr::Var(name) = callee.as_ref() {
                merge_direct_call(name, args, functions, facts, hints, env);
            }
            visit_expr_calls(callee, hints, functions, facts, env);
            for arg in args {
                visit_expr_calls(arg, hints, functions, facts, env);
            }
        }
        Expr::CallNamed(callee, positional, named) => {
            if named.is_empty()
                && let Expr::Var(name) = callee.as_ref()
            {
                merge_direct_call(name, positional, functions, facts, hints, env);
            }
            visit_expr_calls(callee, hints, functions, facts, env);
            for arg in positional {
                visit_expr_calls(arg, hints, functions, facts, env);
            }
            for (_, arg) in named {
                visit_expr_calls(arg, hints, functions, facts, env);
            }
        }
        Expr::Bin(left, _, right)
        | Expr::And(left, right)
        | Expr::Or(left, right)
        | Expr::NullishCoalescing(left, right)
        | Expr::Access(left, right)
        | Expr::OptionalAccess(left, right) => {
            visit_expr_calls(left, hints, functions, facts, env);
            visit_expr_calls(right, hints, functions, facts, env);
        }
        Expr::Unary(_, inner) | Expr::Paren(inner) => visit_expr_calls(inner, hints, functions, facts, env),
        Expr::Conditional(cond, then_expr, else_expr) => {
            visit_expr_calls(cond, hints, functions, facts, env);
            visit_expr_calls(then_expr, hints, functions, facts, env);
            visit_expr_calls(else_expr, hints, functions, facts, env);
        }
        Expr::List(items) => {
            for item in items {
                visit_expr_calls(item, hints, functions, facts, env);
            }
        }
        Expr::Map(entries) => {
            for (key, value) in entries {
                visit_expr_calls(key, hints, functions, facts, env);
                visit_expr_calls(value, hints, functions, facts, env);
            }
        }
        Expr::StructLiteral { fields, .. } => {
            for (_, value) in fields {
                visit_expr_calls(value, hints, functions, facts, env);
            }
        }
        Expr::Range { start, end, step, .. } => {
            for expr in start.iter().chain(end.iter()).chain(step.iter()) {
                visit_expr_calls(expr, hints, functions, facts, env);
            }
        }
        Expr::Select { cases, default_case } => {
            for case in cases {
                visit_select_case(case, hints, functions, facts, env);
            }
            if let Some(default_case) = default_case {
                visit_expr_calls(default_case, hints, functions, facts, env);
            }
        }
        Expr::TemplateString(parts) => {
            for part in parts {
                if let TemplateStringPart::Expr(expr) = part {
                    visit_expr_calls(expr, hints, functions, facts, env);
                }
            }
        }
        Expr::Closure { body, .. } => visit_expr_calls(body, hints, functions, facts, env),
        Expr::Block(_) => {}
        Expr::Match { value, arms } => {
            visit_expr_calls(value, hints, functions, facts, env);
            for MatchArm { pattern, body } in arms {
                visit_pattern_exprs(pattern, hints, functions, facts, env);
                visit_expr_calls(body, hints, functions, facts, env);
            }
        }
        Expr::Var(_) | Expr::Val(_) => {}
    }
}

fn merge_direct_call(
    name: &str,
    args: &[Box<Expr>],
    functions: &HashMap<String, usize>,
    facts: &mut HashMap<String, Vec<ParamFact>>,
    hints: &HashMap<usize, Type>,
    env: &HashMap<String, Type>,
) {
    if functions.get(name).is_some_and(|arity| *arity == args.len())
        && let Some(slots) = facts.get_mut(name)
    {
        for (idx, arg) in args.iter().enumerate() {
            merge_fact(&mut slots[idx], useful_type(arg, hints, env));
        }
    }
}

fn visit_select_case(
    case: &SelectCase,
    hints: &HashMap<usize, Type>,
    functions: &HashMap<String, usize>,
    facts: &mut HashMap<String, Vec<ParamFact>>,
    env: &HashMap<String, Type>,
) {
    match &case.pattern {
        SelectPattern::Recv { channel, .. } => visit_expr_calls(channel, hints, functions, facts, env),
        SelectPattern::Send { channel, value } => {
            visit_expr_calls(channel, hints, functions, facts, env);
            visit_expr_calls(value, hints, functions, facts, env);
        }
    }
    if let Some(guard) = &case.guard {
        visit_expr_calls(guard, hints, functions, facts, env);
    }
    visit_expr_calls(&case.body, hints, functions, facts, env);
}

fn visit_pattern_exprs(
    pattern: &Pattern,
    hints: &HashMap<usize, Type>,
    functions: &HashMap<String, usize>,
    facts: &mut HashMap<String, Vec<ParamFact>>,
    env: &HashMap<String, Type>,
) {
    match pattern {
        Pattern::Guard { pattern, guard } => {
            visit_pattern_exprs(pattern, hints, functions, facts, env);
            visit_expr_calls(guard, hints, functions, facts, env);
        }
        Pattern::Range { start, end, .. } => {
            visit_expr_calls(start, hints, functions, facts, env);
            visit_expr_calls(end, hints, functions, facts, env);
        }
        Pattern::List { patterns, .. } | Pattern::Or(patterns) => {
            for pattern in patterns {
                visit_pattern_exprs(pattern, hints, functions, facts, env);
            }
        }
        Pattern::Map { patterns, .. } => {
            for (_, pattern) in patterns {
                visit_pattern_exprs(pattern, hints, functions, facts, env);
            }
        }
        Pattern::Literal(_) | Pattern::Variable(_) | Pattern::Wildcard => {}
    }
}

fn merge_fact(slot: &mut ParamFact, next: Option<Type>) {
    let Some(next) = next else {
        return;
    };
    match slot {
        ParamFact::Unknown => *slot = ParamFact::Known(next),
        ParamFact::Known(current) if *current == next => {}
        ParamFact::Known(_) | ParamFact::Conflict => *slot = ParamFact::Conflict,
    }
}

fn useful_type(expr: &Expr, hints: &HashMap<usize, Type>, env: &HashMap<String, Type>) -> Option<Type> {
    let key = expr as *const Expr as usize;
    if let Some(ty) = hints.get(&key).and_then(normalize_type) {
        return Some(ty);
    }
    useful_type_without_hints(expr, env, &mut HashSet::new())
}

fn useful_type_without_hints(expr: &Expr, env: &HashMap<String, Type>, seen: &mut HashSet<usize>) -> Option<Type> {
    let key = expr as *const Expr as usize;
    if !seen.insert(key) {
        return None;
    }
    match expr {
        Expr::Val(Val::Int(_)) => Some(Type::Int),
        Expr::List(_) => Some(Type::List(Box::new(Type::Any))),
        Expr::Map(_) => Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any))),
        Expr::Var(name) => env.get(name).cloned(),
        Expr::Paren(inner) => useful_type_without_hints(inner, env, seen),
        Expr::Bin(left, op, right) if op.is_arith() && !matches!(op, BinOp::Div) => {
            let left = useful_type_without_hints(left, env, seen);
            let right = useful_type_without_hints(right, env, seen);
            (left == Some(Type::Int) && right == Some(Type::Int)).then_some(Type::Int)
        }
        _ => None,
    }
}

fn bind_for_pattern_type(pattern: &ForPattern, ty: Type, env: &mut HashMap<String, Type>) {
    if let ForPattern::Variable(name) = pattern {
        env.insert(name.clone(), ty);
    }
}

fn bind_pattern_type(pattern: &Pattern, ty: Type, env: &mut HashMap<String, Type>) {
    if let Pattern::Variable(name) = pattern {
        env.insert(name.clone(), ty);
    }
}

fn normalize_type(ty: &Type) -> Option<Type> {
    match ty {
        Type::Int => Some(Type::Int),
        Type::List(_) => Some(Type::List(Box::new(Type::Any))),
        Type::Map(_, _) => Some(Type::Map(Box::new(Type::Any), Box::new(Type::Any))),
        _ => None,
    }
}
