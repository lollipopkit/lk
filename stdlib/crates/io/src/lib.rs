pub mod file;
pub mod std_io;

pub mod bytes {
    pub use lk_stdlib_bytes::*;
}
pub mod resource {
    pub use lk_stdlib_common::resource::*;
}
pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

use anyhow::Result;
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    vm::RuntimeExport,
};
use lk_stdlib_common::metadata::StdlibModuleMetadata;
use lk_stdlib_common::runtime_native::namespace_export;

#[derive(Debug)]
pub struct IoModule;

impl IoModule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for IoModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for IoModule {
    fn name(&self) -> &str {
        "io"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        namespace_export(&[
            ("std", std_io::IoStdModule::new().runtime_exports()?),
            ("file", file::IoFileModule::new().runtime_exports()?),
        ])
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    lk_stdlib_common::metadata::register_stdlib_module_metadata(metadata())?;
    registry.register_module("io", Box::new(IoModule::new()))
}

pub fn metadata() -> StdlibModuleMetadata {
    lk_stdlib_common::stdlib_module_metadata!(
        io,
        [
            std.flush => Nil,
            std.read_to_string => String,
            std.stderr => RuntimeValue,
            std.stdin => RuntimeValue,
            std.stdout => RuntimeValue,
            std.write => Nil,
            std.writeln => Nil,
        ]
    )
}
