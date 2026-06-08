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
    util::fast_map::fast_hash_map_new,
    val::{HeapStore, HeapValue, RuntimeVal, TypedMap},
    vm::{RuntimeExport, import_runtime_export},
};
use std::sync::Arc;

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
    registry.register_module("net", Box::new(NetModule::new()))
}

fn namespace_export(entries: &[(&'static str, RuntimeExport)]) -> Result<RuntimeExport> {
    let mut heap = HeapStore::new();
    let mut map = fast_hash_map_new();
    for (name, export) in entries {
        map.insert(Arc::<str>::from(*name), import_runtime_export(export, &mut heap)?);
    }
    let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(map))));
    Ok(RuntimeExport::from_value(value, heap))
}
