use anyhow::{Result, bail};
use lk_core::util::fast_map::{FastHashMap, fast_hash_map_new};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry},
    val::{CallableValue, HeapStore, HeapValue, RuntimeVal, TypedMap},
    vm::{Module, NativeArgs, NativeFunction, NativeRuntime, PlainNativeFunction, RuntimeExport, RuntimeModuleState},
};
use lk_stdlib_common::metadata::StdlibModuleMetadata;
use std::sync::{Arc, Mutex};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

use crate::runtime_native::runtime_string_value;

#[derive(Debug)]
pub struct OsModule;

impl Default for OsModule {
    fn default() -> Self {
        Self::new()
    }
}

impl OsModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for OsModule {
    fn name(&self) -> &str {
        "os"
    }

    fn description(&self) -> &str {
        "Operating system interface"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        fn callable(heap: &mut HeapStore, f: PlainNativeFunction, arity: u16) -> RuntimeVal {
            RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
                name: Arc::<str>::from("os::<native>"),
                arity,
                function: NativeFunction::Plain(f),
            })))
        }
        fn key(s: &str) -> Arc<str> {
            Arc::<str>::from(s)
        }

        let mut heap = HeapStore::new();

        // Build outer module map
        let mut entries: FastHashMap<Arc<str>, RuntimeVal> = fast_hash_map_new();
        entries.insert(key("hostname"), callable(&mut heap, hostname, 0));
        entries.insert(key("arch"), callable(&mut heap, arch, 0));
        entries.insert(key("os"), callable(&mut heap, os, 0));
        entries.insert(key("clock"), callable(&mut heap, clock, 0));
        entries.insert(key("time"), callable(&mut heap, time, 0));
        entries.insert(key("epoch"), callable(&mut heap, epoch, 0));

        let value = RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::StringMixed(entries))));
        Ok(RuntimeExport::new(
            value,
            Arc::new(Mutex::new(RuntimeModuleState::new(heap, Vec::new()))),
            Arc::new(Module::default()),
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    lk_stdlib_common::metadata::register_stdlib_module_metadata(metadata())?;
    registry.register_module("os", Box::new(OsModule::new()))
}

pub fn metadata() -> StdlibModuleMetadata {
    lk_stdlib_common::stdlib_module_metadata!(
        os,
        [
            arch => String,
            clock => Float,
            epoch => Int,
            hostname => String,
            os => String,
        ]
    )
}

fn no_args(args: NativeArgs<'_>, name: &str) -> Result<()> {
    if args.len() == 0 {
        Ok(())
    } else {
        bail!("{name}() takes no arguments")
    }
}

fn hostname(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "hostname")?;
    let hostname = std::env::var_os("HOSTNAME")
        .or_else(|| std::env::var_os("COMPUTERNAME"))
        .and_then(|value| value.into_string().ok())
        .unwrap_or_else(|| "localhost".to_string());
    Ok(runtime_string_value(&hostname, runtime.heap_mut()))
}

fn arch(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "arch")?;
    Ok(runtime_string_value(std::env::consts::ARCH, runtime.heap_mut()))
}

fn os(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "os")?;
    Ok(runtime_string_value(std::env::consts::OS, runtime.heap_mut()))
}

fn clock(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "clock")?;
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    static START: AtomicU64 = AtomicU64::new(0);
    static INIT: AtomicBool = AtomicBool::new(false);
    if !INIT.swap(true, Ordering::SeqCst) {
        START.store(epoch_nanos(), Ordering::SeqCst);
    }
    let elapsed_secs = epoch_nanos().wrapping_sub(START.load(Ordering::SeqCst)) as f64 / 1_000_000_000.0;
    Ok(RuntimeVal::Float(elapsed_secs))
}

fn time(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "time")?;
    Ok(RuntimeVal::Int(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64,
    ))
}

fn epoch(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "epoch")?;
    Ok(RuntimeVal::Int(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64,
    ))
}

fn epoch_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
