use crate::llvm::stdlib_catalog::{stdlib_export_path_value, stdlib_module_name};
use crate::stmt::import::{ImportSource, ImportStmt};

use super::{NativeBuiltin, NativeModule, NativeStraightlineValue, native_static_global};

pub(in crate::llvm) fn native_static_global_with_imports(
    imports: &[ImportStmt],
    name: &str,
) -> Option<NativeStraightlineValue> {
    native_static_import_global(imports, name).or_else(|| native_static_global(name))
}

pub(in crate::llvm) fn native_static_import_globals(
    imports: &[ImportStmt],
    global_names: &[String],
) -> Vec<Option<NativeStraightlineValue>> {
    global_names
        .iter()
        .map(|name| native_static_import_global(imports, name))
        .collect()
}

fn native_static_import_global(imports: &[ImportStmt], name: &str) -> Option<NativeStraightlineValue> {
    for import in imports {
        let value = match import {
            ImportStmt::Module { module } if crate::stmt::import::default_module_binding(module) == name => {
                stdlib_module_name(module)
                    .or_else(|| native_example_module_name(module))
                    .map(|module| NativeStraightlineValue::Module(NativeModule::new(module)))
            }
            ImportStmt::ModuleAlias { module, alias } if alias == name => {
                stdlib_module_name(module)
                    .or_else(|| native_example_module_name(module))
                    .map(|module| NativeStraightlineValue::Module(NativeModule::new(module)))
            }
            ImportStmt::File { path } if crate::stmt::import::default_module_binding(path) == name => {
                native_example_module_name(path).map(|module| NativeStraightlineValue::Module(NativeModule::new(module)))
            }
            ImportStmt::Namespace {
                alias,
                source: ImportSource::Module(module),
            } if alias == name => {
                stdlib_module_name(module).map(|module| NativeStraightlineValue::Module(NativeModule::new(module)))
            }
            ImportStmt::Items {
                items,
                source: ImportSource::Module(module),
            } => items.iter().find_map(|item| {
                let binding = item.alias.as_deref().unwrap_or(&item.name);
                if binding != name {
                    return None;
                }
                stdlib_export_path_value(&[module.as_str(), item.name.as_str()])
            }),
            ImportStmt::Items {
                items,
                source: ImportSource::File(path),
            } => items.iter().find_map(|item| {
                let binding = item.alias.as_deref().unwrap_or(&item.name);
                if binding != name {
                    return None;
                }
                native_example_file_export(path, &item.name)
            }),
            ImportStmt::Module { .. }
            | ImportStmt::File { .. }
            | ImportStmt::Namespace { .. }
            | ImportStmt::ModuleAlias { .. } => None,
        };
        if value.is_some() {
            return value;
        }
    }
    None
}

fn native_example_module_name(module: &str) -> Option<&'static str> {
    // Deliberate example-only native mappings. The import binding is normalized
    // through crate::stmt::import::default_module_binding, then mapped to
    // NativeStraightlineValue::Module entries whose exports are fixed in the
    // LLVM native table, including NativeBuiltin::FibIterative.
    match crate::stmt::import::default_module_binding(module).as_str() {
        "fib" => Some("example.fib"),
        "mathlib" => Some("example.mathlib"),
        "greetings" => Some("example.greetings"),
        _ => None,
    }
}

fn native_example_file_export(path: &str, name: &str) -> Option<NativeStraightlineValue> {
    // Keep this in sync with native_example_module_name and the example sources.
    match (crate::stmt::import::default_module_binding(path).as_str(), name) {
        ("fib", "iterative") => Some(NativeStraightlineValue::Builtin(NativeBuiltin::FibIterative)),
        _ => None,
    }
}
