//! Signatures for names a program imports from another file.
//!
//! Without this, `use { f } from "lib";` leaves `f` typed `Any`: the checker
//! walks one program, and a name that came from another file has nothing
//! behind it. Everything downstream then degrades — a range bound, a
//! condition, a cast all need something better than `Any` — and, worse, a call
//! with the wrong number or type of arguments is not checked at all. Such a
//! call reaches the native lowering, which reports "opcode CallDirect is not
//! natively lowerable" and names neither the call nor the reason.
//!
//! What is read is only what the imported file *states*: parameter and return
//! annotations. Inference is not run over the dependency — its own body is its
//! own program's business, and running it here would be both slow and a second
//! place for inference to disagree with itself. An unannotated parameter stays
//! `Any`, exactly as permissive as before.

use std::path::{Path, PathBuf};

use crate::stmt::{ImportSource, ImportStmt, Program, Stmt};
use crate::syntax::{ParseOptions, parse_program_source};
use crate::typ::{FunctionSig, NamedParamSig, TypeChecker};
use crate::val::{FunctionNamedParamType, Type};

/// Registers a signature for every function `program` imports from a file.
///
/// `base_dir` is the directory the importing file lives in, since that is what
/// an import path is relative to. Failures are silent by design: an import
/// that cannot be found or parsed is reported by the stage that actually needs
/// it (macro expansion, or the module loader), and turning a *type* pass into
/// a second place that reports missing files would give the same problem two
/// voices.
pub fn seed_imported_signatures(program: &Program, base_dir: &Path, checker: &mut TypeChecker) {
    for import in &program.statements {
        let Stmt::Import(import) = item_of(import) else {
            continue;
        };
        match import {
            ImportStmt::Items { items, source } => {
                let ImportSource::File(path) = source else {
                    continue;
                };
                let Some(dep) = load(base_dir, path) else {
                    continue;
                };
                for item in items {
                    let bound = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    if let Some((signature, function_type)) = signature_of(&dep, &item.name) {
                        checker.add_function_sig(bound.clone(), signature);
                        checker.add_local_type(bound, function_type);
                    }
                }
            }
            // `use "lib";` and `use * as m from "lib";` bind a namespace, whose
            // members are reached as `m.f`. Member types are a separate
            // mechanism from function signatures, so they are left alone here
            // rather than half-registered under a made-up name.
            _ => continue,
        }
    }
}

/// Is `candidate` inside the importing file's package?
///
/// The package is the nearest ancestor with an `Lk.toml`, or the importing
/// file's own directory when there is none — the same rule the module resolver
/// uses, so the two agree about what "inside" means. `..` is therefore allowed
/// as a way to reach a sibling directory of the same package, and not as a way
/// out of it.
fn within_package(base_dir: &Path, candidate: &Path) -> bool {
    let anchor = base_dir.canonicalize().unwrap_or_else(|_| base_dir.to_path_buf());
    let root = crate::package::find_manifest(&anchor)
        .and_then(|manifest| manifest.parent().map(Path::to_path_buf))
        .unwrap_or(anchor);
    let root = root.canonicalize().unwrap_or(root);
    let resolved = candidate.canonicalize();
    resolved.as_deref().unwrap_or(candidate).starts_with(&root)
}

fn item_of(stmt: &Stmt) -> &Stmt {
    match stmt {
        Stmt::Attributed { item, .. } => item_of(item),
        other => other,
    }
}

/// Mirrors the module resolver's candidates: `p` (already `.lk`), `p.lk`, and
/// `p/mod.lk`, under the importing file's directory.
///
/// And its *boundaries*, which matter more here than the candidate list: this
/// runs during `lk check` and native compilation, before anything is executed,
/// so a path it accepts is a file those commands read. The runtime resolver
/// refuses absolute paths and refuses to leave the package (see
/// `ModuleResolver::resolve_file_path`); reading a file here that a run would
/// refuse to import would make type checking answer questions about a file
/// outside the boundary.
fn load(base_dir: &Path, import_path: &str) -> Option<Program> {
    let raw = Path::new(import_path);
    if !raw.is_relative() {
        return None;
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if raw.extension().and_then(|extension| extension.to_str()) == Some("lk") {
        candidates.push(base_dir.join(raw));
    }
    candidates.push(base_dir.join(raw.with_extension("lk")));
    candidates.push(base_dir.join(raw).join("mod.lk"));
    let path = candidates
        .into_iter()
        .find(|candidate| candidate.exists() && within_package(base_dir, candidate))?;
    let source = std::fs::read_to_string(&path).ok()?;
    parse_program_source(
        &source,
        ParseOptions {
            base_dir: path.parent().map(Path::to_path_buf),
            ..ParseOptions::default()
        },
    )
    .ok()
}

/// The stated signature of a top-level `fn` in `program`.
fn signature_of(program: &Program, name: &str) -> Option<(FunctionSig, Type)> {
    for stmt in &program.statements {
        let Stmt::Function {
            name: declared,
            params,
            param_types,
            named_params,
            return_type,
            ..
        } = item_of(stmt)
        else {
            continue;
        };
        if declared != name {
            continue;
        }
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
        return Some((
            FunctionSig {
                positional,
                named,
                return_type: Some(returns),
                annotated,
            },
            function_type,
        ));
    }
    None
}
