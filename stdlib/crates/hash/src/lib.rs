use anyhow::{Result, bail};
use lk_core::{
    module::{ModuleProvider, ModuleRegistry, RuntimeNativeExport, runtime_export_from_plain_native_entries},
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime, RuntimeExport},
};
use lk_stdlib_bytes::runtime_bytes_or_string_arg;
use lk_stdlib_common::runtime_native::runtime_string_value;
use sha1::Digest as _;

#[derive(Debug, Default)]
pub struct HashModule;

impl HashModule {
    pub fn new() -> Self {
        Self
    }
}

impl ModuleProvider for HashModule {
    fn name(&self) -> &str {
        "hash"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport::plain("sha256", sha256, 1),
                RuntimeNativeExport::plain("sha1", sha1, 1),
                RuntimeNativeExport::plain("crc32", crc32, 1),
                RuntimeNativeExport::plain("fnv64", fnv64, 1),
            ],
            &[],
        ))
    }
}

pub fn register(registry: &mut ModuleRegistry) -> Result<()> {
    registry.register_module("hash", Box::new(HashModule::new()))
}

fn sha256(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let data = data_arg(args, runtime, "hash.sha256()")?;
    Ok(runtime_string_value(
        &format!("{:x}", sha2::Sha256::digest(data.as_ref())),
        runtime.heap_mut(),
    ))
}

fn sha1(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let data = data_arg(args, runtime, "hash.sha1()")?;
    Ok(runtime_string_value(
        &format!("{:x}", sha1::Sha1::digest(data.as_ref())),
        runtime.heap_mut(),
    ))
}

fn crc32(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    let data = data_arg(args, runtime, "hash.crc32()")?;
    Ok(RuntimeVal::Int(crc32fast::hash(data.as_ref()) as i64))
}

fn fnv64(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let data = data_arg(args, runtime, "hash.fnv64()")?;
    let mut hash = OFFSET;
    for byte in data.iter() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    Ok(RuntimeVal::Int(hash as i64))
}

fn data_arg(args: NativeArgs<'_>, runtime: &NativeRuntime<'_>, name: &str) -> Result<std::sync::Arc<[u8]>> {
    expect_arity(args, 1, name)?;
    runtime_bytes_or_string_arg(args.get(0).expect("checked arity"), runtime.heap(), name)
}

fn expect_arity(args: NativeArgs<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        Ok(())
    } else {
        bail!("{name} expects exactly {expected} argument(s)")
    }
}
