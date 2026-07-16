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
    pub(crate) fn build(imports: &[lk_core::stmt::ImportStmt], bundles: &[BundledImport]) -> Self {
        use lk_core::stmt::{ImportSource, ImportStmt};
        let mut env = ImportEnv {
            bundles: bundles.to_vec(),
            ..ImportEnv::default()
        };
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
        env
    }
}
