use anyhow::Result;
use lk_core::module::ModuleRegistry;

use crate::{
    register_stdlib_concurrency_globals, register_stdlib_core_globals, register_stdlib_globals,
    register_stdlib_module_chan, register_stdlib_module_datetime, register_stdlib_module_io,
    register_stdlib_module_iter, register_stdlib_module_json, register_stdlib_module_list, register_stdlib_module_map,
    register_stdlib_module_math, register_stdlib_module_os, register_stdlib_module_stream,
    register_stdlib_module_string, register_stdlib_module_task, register_stdlib_module_tcp,
    register_stdlib_module_time, register_stdlib_module_toml, register_stdlib_module_yaml, register_stdlib_modules,
};

#[unsafe(no_mangle)]
pub extern "Rust" fn lk_stdlib_register_globals(registry: &mut ModuleRegistry) {
    register_stdlib_globals(registry);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn lk_stdlib_register_core_globals(registry: &mut ModuleRegistry) {
    register_stdlib_core_globals(registry);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn lk_stdlib_register_concurrency_globals(registry: &mut ModuleRegistry) {
    register_stdlib_concurrency_globals(registry);
}

#[unsafe(no_mangle)]
pub extern "Rust" fn lk_stdlib_register_modules(registry: &mut ModuleRegistry) -> Result<()> {
    register_stdlib_modules(registry)
}

macro_rules! export_stdlib_module_registrar {
    ($export:ident, $register:ident) => {
        #[unsafe(no_mangle)]
        pub extern "Rust" fn $export(registry: &mut ModuleRegistry) -> Result<()> {
            $register(registry)
        }
    };
}

export_stdlib_module_registrar!(lk_stdlib_register_module_io, register_stdlib_module_io);
export_stdlib_module_registrar!(lk_stdlib_register_module_json, register_stdlib_module_json);
export_stdlib_module_registrar!(lk_stdlib_register_module_yaml, register_stdlib_module_yaml);
export_stdlib_module_registrar!(lk_stdlib_register_module_toml, register_stdlib_module_toml);
export_stdlib_module_registrar!(lk_stdlib_register_module_iter, register_stdlib_module_iter);
export_stdlib_module_registrar!(lk_stdlib_register_module_math, register_stdlib_module_math);
export_stdlib_module_registrar!(lk_stdlib_register_module_string, register_stdlib_module_string);
export_stdlib_module_registrar!(lk_stdlib_register_module_list, register_stdlib_module_list);
export_stdlib_module_registrar!(lk_stdlib_register_module_map, register_stdlib_module_map);
export_stdlib_module_registrar!(lk_stdlib_register_module_datetime, register_stdlib_module_datetime);
export_stdlib_module_registrar!(lk_stdlib_register_module_os, register_stdlib_module_os);
export_stdlib_module_registrar!(lk_stdlib_register_module_tcp, register_stdlib_module_tcp);
export_stdlib_module_registrar!(lk_stdlib_register_module_stream, register_stdlib_module_stream);
export_stdlib_module_registrar!(lk_stdlib_register_module_task, register_stdlib_module_task);
export_stdlib_module_registrar!(lk_stdlib_register_module_chan, register_stdlib_module_chan);
export_stdlib_module_registrar!(lk_stdlib_register_module_time, register_stdlib_module_time);
