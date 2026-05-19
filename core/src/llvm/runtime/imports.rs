use crate::stmt::{ImportSource, ImportStmt};

use super::{
    StdlibRegistrar, push_unique_registrar, register_stdlib_chan_bridge, register_stdlib_concurrency_globals_bridge,
    register_stdlib_datetime_bridge, register_stdlib_io_bridge, register_stdlib_iter_bridge,
    register_stdlib_json_bridge, register_stdlib_list_bridge, register_stdlib_map_bridge, register_stdlib_math_bridge,
    register_stdlib_os_bridge, register_stdlib_stream_bridge, register_stdlib_string_bridge,
    register_stdlib_task_bridge, register_stdlib_tcp_bridge, register_stdlib_time_bridge, register_stdlib_toml_bridge,
    register_stdlib_yaml_bridge,
};

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

pub(super) fn stdlib_registrars_from_imports(imports: &[ImportStmt]) -> Vec<StdlibRegistrar> {
    let mut registrars = Vec::new();
    for name in stdlib_module_names_from_imports(imports) {
        match name.as_str() {
            "io" => push_unique_registrar(&mut registrars, register_stdlib_io_bridge),
            "json" => push_unique_registrar(&mut registrars, register_stdlib_json_bridge),
            "yaml" => push_unique_registrar(&mut registrars, register_stdlib_yaml_bridge),
            "toml" => push_unique_registrar(&mut registrars, register_stdlib_toml_bridge),
            "iter" => push_unique_registrar(&mut registrars, register_stdlib_iter_bridge),
            "math" => push_unique_registrar(&mut registrars, register_stdlib_math_bridge),
            "string" => push_unique_registrar(&mut registrars, register_stdlib_string_bridge),
            "list" => push_unique_registrar(&mut registrars, register_stdlib_list_bridge),
            "map" => push_unique_registrar(&mut registrars, register_stdlib_map_bridge),
            "datetime" => push_unique_registrar(&mut registrars, register_stdlib_datetime_bridge),
            "os" => push_unique_registrar(&mut registrars, register_stdlib_os_bridge),
            "tcp" => push_unique_registrar(&mut registrars, register_stdlib_tcp_bridge),
            "stream" => push_unique_registrar(&mut registrars, register_stdlib_stream_bridge),
            "task" => {
                push_unique_registrar(&mut registrars, register_stdlib_concurrency_globals_bridge);
                push_unique_registrar(&mut registrars, register_stdlib_task_bridge);
            }
            "chan" => {
                push_unique_registrar(&mut registrars, register_stdlib_concurrency_globals_bridge);
                push_unique_registrar(&mut registrars, register_stdlib_chan_bridge);
            }
            "time" => {
                push_unique_registrar(&mut registrars, register_stdlib_concurrency_globals_bridge);
                push_unique_registrar(&mut registrars, register_stdlib_time_bridge);
            }
            _ => {}
        }
    }
    registrars
}

fn push_unique(names: &mut Vec<String>, candidate: &str) {
    if !names.iter().any(|name| name == candidate) {
        names.push(candidate.to_string());
    }
}
