use anyhow::Result;
use lk_core::{
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime},
};
use lk_stdlib_bytes::runtime_bytes_or_string_arg;
use lk_stdlib_common::runtime_native::runtime_string_value;
use sha1::Digest as _;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "hash", docs = "Hash and checksum helpers")]
pub struct HashModule;

#[lk_stdlib_common::stdlib_exports(module = "hash")]
impl HashModule {
    #[stdlib_export(name = "sha256", params(data: Bytes | String), returns = String)]
    fn sha256(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let data = data_arg(args, runtime, "hash.sha256()")?;
        Ok(runtime_string_value(
            &format!("{:x}", sha2::Sha256::digest(data.as_ref())),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(name = "sha1", params(data: Bytes | String), returns = String)]
    fn sha1(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let data = data_arg(args, runtime, "hash.sha1()")?;
        Ok(runtime_string_value(
            &format!("{:x}", sha1::Sha1::digest(data.as_ref())),
            runtime.heap_mut(),
        ))
    }

    #[stdlib_export(name = "crc32", params(data: Bytes | String), returns = Int)]
    fn crc32(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let data = data_arg(args, runtime, "hash.crc32()")?;
        Ok(RuntimeVal::Int(crc32fast::hash(data.as_ref()) as i64))
    }

    #[stdlib_export(name = "fnv64", params(data: Bytes | String), returns = Int)]
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
}

fn data_arg(args: NativeArgs<'_>, runtime: &NativeRuntime<'_>, name: &str) -> Result<std::sync::Arc<[u8]>> {
    runtime_bytes_or_string_arg(args.get(0).expect("checked arity"), runtime.heap(), name)
}
