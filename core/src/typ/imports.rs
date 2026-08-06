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
use crate::typ::declared_signature::signature_of_stmt;
use crate::typ::{FunctionSig, TypeChecker};
use crate::typ::{StructDef, TraitDef, TypeAlias};
use crate::val::Type;

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
                seed_declared_types(&dep, checker);
                seed_impl_methods(&dep, checker);
                for item in items {
                    let bound = item.alias.clone().unwrap_or_else(|| item.name.clone());
                    // A type imported by name is constructible by that name:
                    // the import binds the declaring module's generated
                    // constructor, so `P { … }` has something to call.
                    if checker.registry().get_struct(&item.name).is_some() {
                        checker.registry_mut().mark_constructible_import(&bound, &item.name);
                    }
                    if let Some((signature, function_type)) = signature_of(&dep, &item.name) {
                        checker.add_function_sig(bound.clone(), signature);
                        checker.add_local_type(bound, function_type);
                    }
                }
            }
            // `use * as m from "lib";` binds a namespace whose members are
            // reached as `m.f` — not a free `f`, since two namespaces may each
            // export one.
            ImportStmt::Namespace {
                alias,
                source: ImportSource::File(path),
            } => {
                let Some(dep) = load(base_dir, path) else {
                    continue;
                };
                seed_namespace(alias, &dep, checker);
            }
            // `use "lib";` binds the file's stem, which is the name the module
            // resolver defines it under.
            ImportStmt::File { path } => {
                let Some(namespace) = Path::new(path).file_stem().and_then(|stem| stem.to_str()) else {
                    continue;
                };
                let Some(dep) = load(base_dir, path) else {
                    continue;
                };
                seed_namespace(namespace, &dep, checker);
            }
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

/// Registers every stated function signature in `dep` under `namespace`.
fn seed_namespace(namespace: &str, dep: &Program, checker: &mut TypeChecker) {
    seed_declared_types(dep, checker);
    seed_impl_methods(dep, checker);
    for stmt in &dep.statements {
        let Stmt::Function { name, .. } = item_of(stmt) else {
            continue;
        };
        if let Some((_, function_type)) = signature_of(dep, name) {
            checker.add_imported_member(namespace, name.clone(), function_type);
        }
    }
}

/// Register the `struct`s and `trait`s an imported module declares.
///
/// A type crosses a module boundary by its bare name — `use * as L from
/// "./leaf"; fn passthru(v: Int) -> Deep` names `Deep`, not `L.Deep` — so the
/// importing file's checker has to know it. Only functions were seeded, which
/// went unnoticed while an unknown name silently became `Type::Named`: the
/// annotation type-checked against nothing and the program ran anyway.
fn seed_declared_types(dep: &Program, checker: &mut TypeChecker) {
    for stmt in &dep.statements {
        match item_of(stmt) {
            Stmt::Struct { name, fields } => {
                let fields = fields
                    .iter()
                    .map(|(field, ty)| (field.clone(), ty.clone().unwrap_or(Type::Any)))
                    .collect();
                checker.registry_mut().register_imported_struct(StructDef {
                    name: name.clone(),
                    fields,
                });
            }
            Stmt::Trait { name, methods, .. } => {
                checker.registry_mut().register_trait(TraitDef {
                    name: name.clone(),
                    methods: methods.iter().cloned().collect(),
                });
            }
            // A `type` alias is a declared name like the other two, and crosses
            // a module boundary the same way.
            Stmt::TypeAlias { name, target } => {
                checker.registry_mut().register_type_alias(TypeAlias {
                    name: name.clone(),
                    target_type: target.clone(),
                });
            }
            _ => {}
        }
    }
}

/// Register the method signatures an imported module's `impl` blocks declare.
///
/// A method reaches the checker by being *type-checked*: `stmt_impl`'s `Impl`
/// arm sets the impl's self type and each method body's check calls
/// `add_method_sig`. That only ever happens for the program's own statements, so
/// a method on an imported type was unknown to the checker — and unknown means
/// unchecked, not rejected: the call fell through to `Any`. Same file,
/// `impl Show for Int { fn show(self) -> String … }` and `a.show(1, 2)` was
/// refused ("Method expects 0 arguments"); with the impl one `use` away the same
/// call passed.
///
/// The signature is read from the declaration, not inferred: an imported body is
/// not re-checked here, so an unannotated parameter is `Any` exactly as it is for
/// an imported free function. That keeps this from *tightening* anything — it
/// only makes the arity and the annotated types visible.
fn seed_impl_methods(dep: &Program, checker: &mut TypeChecker) {
    for stmt in &dep.statements {
        let Stmt::Impl {
            target_type, methods, ..
        } = item_of(stmt)
        else {
            continue;
        };
        // Aliases resolve against the importing checker, which already has the
        // dependency's `type` declarations (`seed_declared_types` ran first).
        let self_ty = checker.resolve_aliases(target_type);
        for method in methods {
            let Stmt::Function { name, .. } = item_of(method) else {
                continue;
            };
            let Some((_, function_type)) = signature_of_stmt(item_of(method)) else {
                continue;
            };
            checker.add_method_sig(&self_ty, name, function_type);
        }
    }
}

/// The stated signature of a top-level `fn` in `program`.
fn signature_of(program: &Program, name: &str) -> Option<(FunctionSig, Type)> {
    for stmt in &program.statements {
        let Stmt::Function { name: declared, .. } = item_of(stmt) else {
            continue;
        };
        if declared != name {
            continue;
        }
        return signature_of_stmt(item_of(stmt));
    }
    None
}
