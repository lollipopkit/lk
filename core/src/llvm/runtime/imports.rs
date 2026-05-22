use crate::stmt::{ImportSource, ImportStmt};

pub(super) fn stdlib_module_names_from_imports(imports: &[ImportStmt]) -> Vec<String> {
    let mut names = Vec::new();
    for import in imports {
        match import {
            ImportStmt::Module { module } | ImportStmt::ModuleAlias { module, .. } => {
                push_unique(&mut names, module);
            }
            ImportStmt::Items {
                source: ImportSource::Module(module),
                ..
            }
            | ImportStmt::Namespace {
                source: ImportSource::Module(module),
                ..
            } => {
                push_unique(&mut names, module);
            }
            ImportStmt::File { .. }
            | ImportStmt::Items {
                source: ImportSource::File(_),
                ..
            }
            | ImportStmt::Namespace {
                source: ImportSource::File(_),
                ..
            } => {}
        }
    }
    names
}

#[cfg(test)]
pub(super) fn imports_need_concurrency_globals(imports: &[ImportStmt]) -> bool {
    stdlib_module_names_from_imports(imports)
        .iter()
        .any(|name| matches!(name.as_str(), "task" | "chan" | "time"))
}

fn push_unique(names: &mut Vec<String>, candidate: &str) {
    if !names.iter().any(|name| name == candidate) {
        names.push(candidate.to_string());
    }
}
