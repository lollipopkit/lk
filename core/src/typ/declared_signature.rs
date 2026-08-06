//! A `fn` declaration's *stated* signature — read, never inferred.
//!
//! Two callers need this before any body is checked, for the same reason: a
//! signature has to be visible from a call site the ordered walk has not
//! reached yet. `typ::imports` needs it for an imported `impl`, and
//! `Program::predeclare_impl_method_signatures` for a local one below its call.
//!
//! It lives here rather than in `typ::imports` because that module is `std`
//! only (it reads files) while this reads nothing but the AST — and a no_std
//! build of `lk-core` compiles the local pre-pass too.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use crate::stmt::Stmt;
use crate::typ::{FunctionSig, NamedParamSig};
use crate::val::{FunctionNamedParamType, Type};

/// The stated signature of one `fn` declaration, wherever it stands — top level
/// or inside an `impl`, where the receiver is simply its first parameter.
///
/// Shared with the program's own `impl` pre-pass (`Program::predeclare_impl_
/// method_signatures`): an imported impl and a local one below its call site
/// are the same problem — the signature has to be readable from the
/// declaration, before any body is checked.
pub(crate) fn signature_of_stmt(stmt: &Stmt) -> Option<(FunctionSig, Type)> {
    let Stmt::Function {
        params,
        param_types,
        named_params,
        return_type,
        ..
    } = stmt
    else {
        return None;
    };
    let positional: Vec<Type> = (0..params.len())
        .map(|i| param_types.get(i).cloned().flatten().unwrap_or(Type::Any))
        .collect();
    let annotated: Vec<bool> = (0..params.len())
        .map(|i| param_types.get(i).cloned().flatten().is_some())
        .collect();
    let named: Vec<NamedParamSig> = named_params
        .iter()
        .map(|param| NamedParamSig {
            name: param.name.clone(),
            ty: param.type_annotation.clone().unwrap_or(Type::Any),
            has_default: param.default.is_some(),
        })
        .collect();
    let returns = return_type.clone().unwrap_or(Type::Any);
    let named_annotations: Vec<FunctionNamedParamType> = named
        .iter()
        .map(|param| FunctionNamedParamType {
            name: param.name.clone(),
            ty: param.ty.clone(),
            has_default: param.has_default,
        })
        .collect();
    let function_type = Type::Function {
        params: positional.clone(),
        named_params: named_annotations,
        return_type: Box::new(returns.clone()),
    };
    Some((
        FunctionSig {
            positional,
            named,
            return_type: Some(returns),
            annotated,
        },
        function_type,
    ))
}
