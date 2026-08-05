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
                            // Functions only, and that is now the whole of it:
                            // a renamed *constant* never reaches here, because
                            // the bundler folds its value into the reads of
                            // both names before this runs.
                            //
                            // It used to reach here and bind nothing — a
                            // `const` is not in `fns` — so `use { SIZE as
                            // TSS_SIZE }` left a `GetGlobal` of a slot nothing
                            // initialises: an error under `compile object:` and
                            // a fall back to the VM otherwise, while the
                            // unrenamed `SIZE` worked because bundling flattens
                            // a module's constants under their own names. See
                            // `collect_renamed_file_items` in the CLI's
                            // bundler, which is where the fold learns the other
                            // name.
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
                // `use math;` binds the module under its own name — the same
                // binding `use math as math;` makes. It was an empty arm, so
                // the lowering could not tell an imported module from a global
                // that happens to share its name: `chan` is both (a bare
                // constructor function *and* a module), and `chan.new(1)`
                // therefore lowered natively whether or not the file imported
                // it, while the VM refused the unimported spelling.
                ImportStmt::Module { module } => {
                    env.module_aliases.insert(module.clone(), module.clone());
                }
            }
        }
        Ok(env)
    }
}
