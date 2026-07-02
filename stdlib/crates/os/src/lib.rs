use anyhow::{Result, bail};
use lk_core::{
    val::RuntimeVal,
    vm::{NativeArgs, NativeRuntime},
};

pub mod runtime_native {
    pub use lk_stdlib_common::runtime_native::*;
}

use crate::runtime_native::runtime_string_value;

#[derive(Debug, Default, lk_stdlib_common::StdlibModule)]
#[stdlib_module(name = "os", docs = "Operating system interface")]
pub struct OsModule;

#[lk_stdlib_common::stdlib_exports(module = "os")]
impl OsModule {
    #[stdlib_export(params(), returns = String)]
    fn hostname(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        no_args(args, "hostname")?;
        let hostname = std::env::var_os("HOSTNAME")
            .or_else(|| std::env::var_os("COMPUTERNAME"))
            .and_then(|value| value.into_string().ok())
            .unwrap_or_else(|| "localhost".to_string());
        Ok(runtime_string_value(&hostname, runtime.heap_mut()))
    }

    #[stdlib_export(params(), returns = String)]
    fn arch(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        no_args(args, "arch")?;
        Ok(runtime_string_value(std::env::consts::ARCH, runtime.heap_mut()))
    }

    #[stdlib_export(name = "os", params(), returns = String)]
    fn os_name(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        os(args, runtime)
    }

    #[stdlib_export(params(), returns = Float)]
    fn clock(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        no_args(args, "clock")?;
        use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
        static START: AtomicU64 = AtomicU64::new(0);
        static INIT: AtomicBool = AtomicBool::new(false);
        if INIT
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
        {
            START.store(epoch_nanos(), Ordering::SeqCst);
        } else {
            while START.load(Ordering::SeqCst) == 0 {
                std::thread::yield_now();
            }
        }
        let elapsed_secs = epoch_nanos().wrapping_sub(START.load(Ordering::SeqCst)) as f64 / 1_000_000_000.0;
        Ok(RuntimeVal::Float(elapsed_secs))
    }

    #[stdlib_export(params(), returns = Int)]
    fn time(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        no_args(args, "time")?;
        Ok(RuntimeVal::Int(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64,
        ))
    }

    #[stdlib_export(params(), returns = Int)]
    fn epoch(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        no_args(args, "epoch")?;
        Ok(RuntimeVal::Int(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64,
        ))
    }
}

fn no_args(args: NativeArgs<'_>, name: &str) -> Result<()> {
    if args.is_empty() {
        Ok(())
    } else {
        bail!("{name}() takes no arguments")
    }
}

fn os(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
    no_args(args, "os")?;
    Ok(runtime_string_value(std::env::consts::OS, runtime.heap_mut()))
}

fn epoch_nanos() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos() as u64
}
