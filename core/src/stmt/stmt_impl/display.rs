use super::{ForPattern, Stmt};
use crate::{
    op::BinOp,
    stmt::{ImportSource, ImportStmt},
};
use std::fmt::{self, Display};

impl Display for Stmt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Stmt::Import(import_stmt) => {
                write!(f, "{};", format_import_stmt(import_stmt))
            }
            Stmt::If {
                condition,
                then_stmt,
                else_stmt,
            } => {
                if let Some(else_stmt) = else_stmt {
                    write!(f, "if ({}) {} else {}", condition, then_stmt, else_stmt)
                } else {
                    write!(f, "if ({}) {}", condition, then_stmt)
                }
            }
            Stmt::IfLet {
                pattern,
                value,
                then_stmt,
                else_stmt,
            } => {
                if let Some(else_stmt) = else_stmt {
                    write!(f, "if let {} = {} {} else {}", pattern, value, then_stmt, else_stmt)
                } else {
                    write!(f, "if let {} = {} {}", pattern, value, then_stmt)
                }
            }
            Stmt::While { condition, body } => {
                write!(f, "while ({}) {}", condition, body)
            }
            Stmt::WhileLet { pattern, value, body } => {
                write!(f, "while let {} = {} {}", pattern, value, body)
            }
            Stmt::For {
                pattern,
                iterable,
                body,
            } => {
                write!(f, "for {} in {} {}", format_pattern(pattern), iterable, body)
            }
            Stmt::Let {
                pattern,
                type_annotation,
                value,
                span: _,
                is_const,
            } => {
                let kw = if *is_const { "const" } else { "let" };
                if let Some(typ) = type_annotation {
                    write!(f, "{} {}: {:?} = {};", kw, pattern, typ, value)
                } else {
                    write!(f, "{} {} = {};", kw, pattern, value)
                }
            }
            Stmt::Assign { name, value, span: _ } => {
                write!(f, "{} = {};", name, value)
            }
            Stmt::CompoundAssign {
                name,
                op,
                value,
                span: _,
            } => {
                let op_str = match op {
                    BinOp::Add => "+=",
                    BinOp::Sub => "-=",
                    BinOp::Mul => "*=",
                    BinOp::Div => "/=",
                    BinOp::Mod => "%=",
                    _ => "?=", // Should not happen for compound assignment
                };
                write!(f, "{} {} {};", name, op_str, value)
            }
            Stmt::Define { name, value } => {
                write!(f, "{} = {};", name, value)
            }
            Stmt::Break => {
                write!(f, "break;")
            }
            Stmt::Continue => {
                write!(f, "continue;")
            }
            Stmt::Return { value } => {
                if let Some(expr) = value {
                    write!(f, "return {};", expr)
                } else {
                    write!(f, "return;")
                }
            }
            Stmt::Struct { name, fields } => {
                // Format as: struct Name { f: T, g: ?U }
                write!(f, "struct {} {{", name)?;
                for (i, (k, ty)) in fields.iter().enumerate() {
                    if i > 0 {
                        write!(f, ", ")?;
                    }
                    if let Some(t) = ty {
                        write!(f, "{}: {}", k, t.display())?;
                    } else {
                        write!(f, "{}", k)?;
                    }
                }
                write!(f, "}}")
            }
            Stmt::TypeAlias { name, target } => {
                write!(f, "type {} = {};", name, target.display())
            }
            Stmt::Trait { name, methods } => {
                write!(f, "trait {} {{", name)?;
                for (i, (m, ty)) in methods.iter().enumerate() {
                    if i > 0 {
                        write!(f, " ")?;
                    }
                    write!(f, "fn {}: {};", m, ty.display())?;
                }
                write!(f, "}}")
            }
            Stmt::Impl {
                trait_name,
                target_type,
                methods,
            } => {
                write!(f, "impl {} for {} {{", trait_name, target_type.display())?;
                for m in methods {
                    if let Stmt::Function {
                        name,
                        params,
                        param_types,
                        return_type,
                        ..
                    } = m
                    {
                        // summarize method signature only
                        write!(f, " fn {}(", name)?;
                        for (i, p) in params.iter().enumerate() {
                            if i > 0 {
                                write!(f, ", ")?;
                            }
                            if let Some(t) = param_types.get(i).cloned().flatten() {
                                write!(f, "{}: {}", p, t.display())?;
                            } else {
                                write!(f, "{}", p)?;
                            }
                        }
                        if let Some(rt) = return_type {
                            write!(f, ") -> {} {{ ... }}", rt.display())?;
                        } else {
                            write!(f, ") {{ ... }}")?;
                        }
                    }
                }
                write!(f, " }}")
            }
            Stmt::Function {
                name,
                params,
                param_types,
                return_type,
                body,
                ..
            } => {
                // Format parameters with optional types
                let parts: Vec<String> = params
                    .iter()
                    .enumerate()
                    .map(|(i, p)| match param_types.get(i).and_then(|t| t.clone()) {
                        Some(ty) => format!("{}: {}", p, ty.display()),
                        None => p.clone(),
                    })
                    .collect();
                // Format return type and elide full body to avoid huge/recursive prints
                let body_summary = if let Stmt::Block { statements } = &**body {
                    format!("... ({} statements) ...", statements.len())
                } else {
                    "...".to_string()
                };
                if let Some(ret) = return_type {
                    write!(
                        f,
                        "fn {}({}) -> {} {{ {} }}",
                        name,
                        parts.join(", "),
                        ret.display(),
                        body_summary
                    )
                } else {
                    write!(f, "fn {}({}) {{ {} }}", name, parts.join(", "), body_summary)
                }
            }
            Stmt::Expr(expr) => {
                write!(f, "{};", expr)
            }
            Stmt::Block { statements } => {
                writeln!(f, "{{")?;
                for stmt in statements {
                    writeln!(f, "  {}", stmt)?;
                }
                write!(f, "}}")
            }
            Stmt::Empty => {
                write!(f, ";")
            }
        }
    }
}

/// Helper function to format import statements for display
fn format_import_stmt(import: &ImportStmt) -> String {
    match import {
        ImportStmt::Module { module } => {
            format!("import {}", module)
        }
        ImportStmt::File { path } => {
            format!("import \"{}\"", path)
        }
        ImportStmt::Items { items, source } => {
            let items_str = items
                .iter()
                .map(|item| {
                    if let Some(alias) = &item.alias {
                        format!("{} as {}", item.name, alias)
                    } else {
                        item.name.clone()
                    }
                })
                .collect::<Vec<_>>()
                .join(", ");

            let source_str = match source {
                ImportSource::Module(name) => name.clone(),
                ImportSource::File(path) => format!("\"{}\"", path),
            };

            format!("import {{ {} }} from {}", items_str, source_str)
        }
        ImportStmt::Namespace { alias, source } => {
            let source_str = match source {
                ImportSource::Module(name) => name.clone(),
                ImportSource::File(path) => format!("\"{}\"", path),
            };
            format!("import * as {} from {}", alias, source_str)
        }
        ImportStmt::ModuleAlias { module, alias } => {
            format!("import {} as {}", module, alias)
        }
    }
}

/// Helper function to format patterns for display
fn format_pattern(pattern: &ForPattern) -> String {
    match pattern {
        ForPattern::Variable(name) => name.clone(),
        ForPattern::Ignore => "_".to_string(),
        ForPattern::Tuple(patterns) => {
            let patterns_str = patterns.iter().map(format_pattern).collect::<Vec<_>>().join(", ");
            format!("({})", patterns_str)
        }
        ForPattern::Array { patterns, rest } => {
            let mut parts = patterns.iter().map(format_pattern).collect::<Vec<_>>();
            if let Some(rest_var) = rest {
                parts.push(format!("..{}", rest_var));
            } else if rest.is_some() {
                parts.push("..".to_string());
            }
            format!("[{}]", parts.join(", "))
        }
        ForPattern::Object(entries) => {
            let parts = entries
                .iter()
                .map(|(k, v)| format!("\"{}\": {}", k, format_pattern(v)))
                .collect::<Vec<_>>()
                .join(", ");
            format!("{{{}}}", parts)
        }
    }
}
