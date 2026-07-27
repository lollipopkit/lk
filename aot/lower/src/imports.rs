/// One compile-time-bundled file import (`use "../general/fib"`): the CLI
/// appended the dep's functions to the artifact's function table (indices
/// rewritten) and reports each of its top-level `fn` names here.
#[derive(Debug, Clone, Default)]
pub struct BundledImport {
    /// The import path exactly as written in the source.
    pub path: String,
    /// Top-level `fn` name → merged function index.
    pub fns: std::collections::HashMap<String, u32>,
}

/// Import-derived name bindings, resolved once from `artifact.imports` (+ the
/// CLI's bundled file modules): how a `GetGlobal` name maps to a module
/// object, a module member, or a bundled user function.
#[derive(Debug, Clone, Default)]
pub(crate) struct ImportEnv {
    /// `use math as m;` / `use * as sm from string;` → alias → module.
    pub(crate) module_aliases: std::collections::HashMap<String, String>,
    /// `use { abs, sqrt as s } from math;` → bound name → (module, member).
    pub(crate) module_items: std::collections::HashMap<String, (String, String)>,
    /// `use "path";` → binding (file stem) → index into `bundles`.
    pub(crate) file_namespaces: std::collections::HashMap<String, usize>,
    /// `use { f } from "path";` → bound name → merged function index.
    pub(crate) file_items: std::collections::HashMap<String, u32>,
    pub(crate) bundles: Vec<BundledImport>,
}

impl ImportEnv {
    pub(crate) fn build(
        imports: &[lk_core::stmt::ImportStmt],
        bundles: &[BundledImport],
    ) -> Result<Self, crate::Unsupported> {
        use lk_core::stmt::{ImportSource, ImportStmt};
        let mut env = ImportEnv {
            bundles: bundles.to_vec(),
            ..ImportEnv::default()
        };
        // Every bundled module's top-level functions are reachable by name.
        // The merge puts them all in one namespace, and a *nested* import — a
        // driver that imports another driver — has no other way to resolve:
        // its `GetGlobal` names never appear in the importing file's own
        // import list. Explicit imports are processed after this, so an alias
        // still wins where the two disagree.
        //
        // Two bundles defining the same name is refused rather than resolved:
        // whichever came last in iteration order would silently win for every
        // nested read, so a driver could end up calling another driver's
        // `init`/`read`/`write` with nothing said. Under the VM each module
        // keeps its own namespace, so this is a divergence, and the rule
        // everywhere else in this bundler applies — report the cause, do not
        // pick one. (The same file reached under two import paths appears
        // twice with identical indices; that is not a collision.)
        for bundle in bundles {
            for (name, fidx) in &bundle.fns {
                if let Some(existing) = env.file_items.get(name)
                    && existing != fidx
                {
                    return Err(crate::Unsupported::BundledNameCollision { name: name.clone() });
                }
                env.file_items.insert(name.clone(), *fidx);
            }
        }
        let bundle_by_path = |path: &str| bundles.iter().position(|b| b.path == path);
        for import in imports {
            match import {
                ImportStmt::ModuleAlias { module, alias } => {
                    env.module_aliases.insert(alias.clone(), module.clone());
                }
                ImportStmt::Namespace { alias, source } => match source {
                    ImportSource::Module(module) => {
                        env.module_aliases.insert(alias.clone(), module.clone());
                    }
                    ImportSource::File(path) => {
                        if let Some(b) = bundle_by_path(path) {
                            env.file_namespaces.insert(alias.clone(), b);
                        }
                    }
                },
                ImportStmt::Items { items, source } => {
                    for item in items {
                        let bound = item.alias.clone().unwrap_or_else(|| item.name.clone());
                        match source {
                            ImportSource::Module(module) => {
                                env.module_items.insert(bound, (module.clone(), item.name.clone()));
                            }
                            // Functions only, which is a real limit and not an
                            // obvious one: `use { SIZE as TSS_SIZE } from
                            // "drivers/tss"` binds nothing here, because a
                            // `const` is not in `fns`. The *unrenamed* form
                            // works by accident — bundling flattens the
                            // module's constants into the program's globals
                            // under their own names, so `SIZE` resolves as an
                            // ordinary global and `TSS_SIZE` resolves as
                            // nothing.
                            //
                            // Not a wrong answer: the read rejects with
                            // "does not resolve to anything natively
                            // lowerable", which is an error under `compile
                            // object:` and a fall back to the VM otherwise.
                            // The VM does bind it, so the two backends differ
                            // in *coverage*, not in what they compute.
                            // TODO: carry each bundle's top-level constants
                            // (name → global slot) alongside `fns` and bind the
                            // alias to the slot, so a renamed constant import
                            // lowers like a renamed function one.
                            ImportSource::File(path) => {
                                if let Some(fidx) = bundle_by_path(path)
                                    .and_then(|b| bundles[b].fns.get(&item.name))
                                    .copied()
                                {
                                    env.file_items.insert(bound, fidx);
                                }
                            }
                        }
                    }
                }
                ImportStmt::File { path } => {
                    let stem = std::path::Path::new(path)
                        .file_stem()
                        .and_then(|s| s.to_str())
                        .unwrap_or("module")
                        .to_string();
                    if let Some(b) = bundle_by_path(path) {
                        env.file_namespaces.insert(stem, b);
                    }
                }
                ImportStmt::Module { .. } => {}
            }
        }
        Ok(env)
    }
}
