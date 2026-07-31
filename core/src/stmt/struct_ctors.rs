//! A constructor function beside every `struct`, so an imported type can be
//! built by the module that owns it.
//!
//! `module.Type { … }` used to be a syntax error and `use { Pt } from "types"`
//! could not reach a type at all, so a module that declared a type could not
//! let its users make one — every such module had to hand-write a `make`.
//!
//! The reason it could not simply be allowed is that a type's identity carries
//! its defining module (`val::TypeScope`): a `Pt` built in the importer is *not*
//! the `Pt` that `impl Norm for Pt` was registered against, and it would not
//! dispatch. `NewObject` names only the type, and the executor scopes it to
//! whichever module is running.
//!
//! So the object is built *by the defining module*: each `struct S { a, b }`
//! also gets
//!
//! ```lk
//! fn S$new({a: A, b: B}) -> S { return S { a: a, b: b }; }
//! ```
//!
//! and `m.S { a: 1, b: 2 }` is parse-time sugar for `m.S$new(a: 1, b: 2)` (see
//! `ast::parser`). The call runs inside `m`, so the scope, the declaration's
//! field order and trait dispatch are all simply right — no new opcode, no
//! artifact change, and nothing for the AOT lowering to learn.
//!
//! **Named parameters, not positional**, so the caller needs no knowledge of
//! the declaration: it passes the same `field: value` pairs the literal is
//! written with, and a missing or misspelled one is the callee's own arity
//! error rather than something this pass has to check.
//!
//! `$` is untokenizable, so the name cannot collide with anything a program can
//! write — the same trick `try$call` and `select$block` use.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use crate::expr::Expr;
use crate::stmt::{NamedParamDecl, Stmt};

/// The constructor name for `struct S`.
pub fn constructor_name(struct_name: &str) -> String {
    alloc::format!("{struct_name}$new")
}

/// The struct a function constructs, if its name is a constructor's.
///
/// The inverse of [`constructor_name`], and the only place the `$new` spelling
/// is decoded. Three things need it and none of them may spell it themselves:
/// the type checker (so a field error says *field*, not "named argument"), the
/// AOT lowering (so the call's result carries the struct's type identity, which
/// is what makes a method on it devirtualize), and this module.
pub fn constructed_struct_name(function_name: &str) -> Option<&str> {
    function_name.strip_suffix("$new")
}

/// Registers the generated constructor needs beyond one per field: the type
/// name `NewObject` reads, the object being built, and the scratch each field
/// value passes through.
const CONSTRUCTOR_OVERHEAD_REGISTERS: usize = 3;

/// The widest struct that can have a constructor, and therefore exist.
pub const MAX_STRUCT_FIELDS: usize = (u8::MAX as usize + 1) - CONSTRUCTOR_OVERHEAD_REGISTERS;

/// Adds a constructor function after every top-level `struct` declaration.
///
/// The constructor takes one *named parameter* per field, and parameters are
/// locals — so the width of a struct is bounded by the register file, and the
/// bound belongs here rather than wherever the generated body happens to run
/// out. A 254-field `struct` used to compile to "this function needs more than
/// 256 registers … split the body into smaller functions", pointing at a body
/// the program does not contain and offering advice that cannot be followed:
/// the declaration is one statement and emits no code of its own.
pub fn add_struct_constructors(statements: &mut Vec<Box<Stmt>>) -> Result<(), String> {
    let mut out: Vec<Box<Stmt>> = Vec::with_capacity(statements.len());
    for stmt in statements.drain(..) {
        let ctor = match struct_declaration(&stmt) {
            Some((name, fields)) if fields.len() > MAX_STRUCT_FIELDS => {
                return Err(alloc::format!(
                    "struct `{name}` has {} fields, and {MAX_STRUCT_FIELDS} is the most one can have: \
                     building it takes a generated constructor with one parameter per field, and a \
                     function's parameters share the same 256 registers as its temporaries. Split the \
                     type, or hold this many values in a map",
                    fields.len()
                ));
            }
            Some((name, fields)) => Some(constructor_for(name, fields)),
            None => None,
        };
        out.push(stmt);
        if let Some(ctor) = ctor {
            out.push(Box::new(ctor));
        }
    }
    *statements = out;
    Ok(())
}

/// A struct declaration's name and fields, as the AST holds them.
type StructDecl<'a> = (&'a str, &'a [(String, Option<crate::val::Type>)]);

fn struct_declaration(stmt: &Stmt) -> Option<StructDecl<'_>> {
    match stmt {
        Stmt::Attributed { item, .. } => struct_declaration(item),
        Stmt::Struct { name, fields } => Some((name.as_str(), fields.as_slice())),
        _ => None,
    }
}

fn constructor_for(name: &str, fields: &[(String, Option<crate::val::Type>)]) -> Stmt {
    let named_params = fields
        .iter()
        .map(|(field, ty)| NamedParamDecl {
            name: field.clone(),
            type_annotation: ty.clone(),
            default: None,
        })
        .collect();
    let literal_fields = fields
        .iter()
        .map(|(field, _)| (field.clone(), Box::new(Expr::Var(field.clone()))))
        .collect();
    Stmt::Function {
        name: constructor_name(name),
        params: Vec::new(),
        param_types: Vec::new(),
        named_params,
        return_type: Some(crate::val::Type::Named(name.to_string())),
        body: Box::new(Stmt::Block {
            statements: vec![Box::new(Stmt::Return {
                value: Some(Box::new(Expr::StructLiteral {
                    name: name.to_string(),
                    fields: literal_fields,
                })),
            })],
        }),
    }
}
