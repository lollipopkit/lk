pub mod socket;
pub mod tcp;
pub mod udp;

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
pub struct NetModule;

impl NetModule {
    pub fn new() -> Self {
        Self
    }
}

impl Default for NetModule {
    fn default() -> Self {
        Self::new()
    }
}

impl ModuleProvider for NetModule {
    fn name(&self) -> &str {
        "net"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        namespace_export(&[
            ("socket", socket::NetSocketModule::new().runtime_exports()?),
            ("tcp", tcp::NetTcpModule::new().runtime_exports()?),
            ("udp", udp::NetUdpModule::new().runtime_exports()?),
        ])
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    lk_stdlib_common::metadata::register_stdlib_module_metadata(metadata())?;
    registry.register_module("net", Box::new(NetModule::new()))
}

pub fn metadata() -> StdlibModuleMetadata {
    lk_stdlib_common::stdlib_module_metadata!(
        net,
        [
            socket.addr => String,
            tcp.close => Bool,
            tcp.connect => RuntimeValue,
            tcp.read => RuntimeValue,
            tcp.write => Int,
        ]
    )
}
