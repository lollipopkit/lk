use anyhow::{Result, anyhow, bail};
use lk_core::{
    module::{Module, ModuleRegistry, RuntimeNativeExport32, runtime_export_from_plain_native_entries},
    val::{HeapStore, RuntimeVal},
    vm::{NativeArgs32, NativeEntry32, NativeRuntime32, RuntimeExport32},
};
use std::io::{BufRead, Read, Write};

use crate::runtime_native::{runtime_display_value, runtime_string_arg, runtime_string_value};

#[derive(Debug)]
pub struct IoModule;

impl Default for IoModule {
    fn default() -> Self {
        Self::new()
    }
}

impl IoModule {
    pub fn new() -> Self {
        Self
    }
}

impl Module for IoModule {
    fn name(&self) -> &str {
        "io"
    }

    fn register(&self, _registry: &mut ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn runtime_exports(&self) -> Result<RuntimeExport32> {
        Ok(runtime_export_from_plain_native_entries(
            &[
                RuntimeNativeExport32::plain("read", mod_read32, 0),
                RuntimeNativeExport32::plain("stdin_read", stdin_read32, NativeEntry32::VARIADIC),
                RuntimeNativeExport32::plain("stdin_read_line", stdin_read_line32, 0),
                RuntimeNativeExport32::plain("stdin_read_all", stdin_read_all32, 0),
                RuntimeNativeExport32::plain("stdin_flush", stdin_flush32, 0),
                RuntimeNativeExport32::plain("stdout_write", stdout_write32, 1),
                RuntimeNativeExport32::plain("stdout_writeln", stdout_writeln32, 1),
                RuntimeNativeExport32::plain("stdout_flush", stdout_flush32, 0),
                RuntimeNativeExport32::plain("stderr_write", stderr_write32, 1),
                RuntimeNativeExport32::plain("stderr_writeln", stderr_writeln32, 1),
                RuntimeNativeExport32::plain("stderr_flush", stderr_flush32, 0),
            ],
            &[],
        ))
    }
}

fn expect_arity(args: NativeArgs32<'_>, expected: usize, name: &str) -> Result<()> {
    if args.len() == expected {
        return Ok(());
    }
    bail!(
        "{name} takes exactly {expected} argument{}",
        if expected == 1 { "" } else { "s" }
    )
}

fn runtime_display_arg(value: &RuntimeVal, heap: &HeapStore, name: &str) -> Result<String> {
    match runtime_string_arg(value, heap, name) {
        Ok(value) => Ok(value.to_string()),
        Err(_) => runtime_display_value(value, heap),
    }
}

fn stdin_read32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    if args.len() > 1 {
        bail!("stdin_read() takes at most 1 argument: [bytes]");
    }
    if args.is_empty() {
        return read_stdin_line_into(runtime);
    }
    let bytes = match args.get(0).expect("checked arity") {
        RuntimeVal::Int(value) if *value >= 0 => *value as usize,
        other => bail!("bytes must be a non-negative integer, got {:?}", other.kind()),
    };
    if bytes == 0 {
        return Ok(runtime_string_value("", runtime.heap_mut()));
    }
    let mut buffer = vec![0u8; bytes];
    match std::io::stdin().lock().read(&mut buffer) {
        Ok(0) => Ok(RuntimeVal::Nil),
        Ok(read) => {
            buffer.truncate(read);
            match String::from_utf8(buffer) {
                Ok(value) => Ok(runtime_string_value(&value, runtime.heap_mut())),
                Err(_) => Ok(RuntimeVal::Nil),
            }
        }
        Err(err) => Err(anyhow!("stdin read error: {err}")),
    }
}

fn read_stdin_line_into(runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    let mut handle = std::io::stdin().lock();
    let mut line = String::new();
    match handle.read_line(&mut line) {
        Ok(0) => Ok(RuntimeVal::Nil),
        Ok(_) => {
            if line.ends_with('\n') {
                line.pop();
                if line.ends_with('\r') {
                    line.pop();
                }
            }
            Ok(runtime_string_value(&line, runtime.heap_mut()))
        }
        Err(err) => Err(anyhow!("stdin read error: {err}")),
    }
}

fn stdin_read_line32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "stdin_read_line()")?;
    read_stdin_line_into(runtime)
}

fn stdin_read_all32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "stdin_read_all()")?;
    let mut value = String::new();
    std::io::stdin()
        .lock()
        .read_to_string(&mut value)
        .map_err(|err| anyhow!("stdin read error: {err}"))?;
    Ok(runtime_string_value(&value, runtime.heap_mut()))
}

fn stdin_flush32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "stdin_flush()")?;
    Ok(RuntimeVal::Bool(true))
}

fn stdout_write32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stdout_write()")?;
    let data = runtime_display_arg(args.get(0).expect("checked arity"), runtime.heap(), "stdout_write data")?;
    std::io::stdout()
        .write_all(data.as_bytes())
        .map_err(|err| anyhow!("stdout write error: {err}"))?;
    Ok(RuntimeVal::Bool(true))
}

fn stdout_writeln32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stdout_writeln()")?;
    let data = runtime_display_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "stdout_writeln data",
    )?;
    writeln!(std::io::stdout(), "{data}").map_err(|err| anyhow!("stdout write error: {err}"))?;
    Ok(RuntimeVal::Bool(true))
}

fn stdout_flush32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "stdout_flush()")?;
    std::io::stdout()
        .flush()
        .map_err(|err| anyhow!("stdout flush error: {err}"))?;
    Ok(RuntimeVal::Bool(true))
}

fn stderr_write32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stderr_write()")?;
    let data = runtime_display_arg(args.get(0).expect("checked arity"), runtime.heap(), "stderr_write data")?;
    std::io::stderr()
        .write_all(data.as_bytes())
        .map_err(|err| anyhow!("stderr write error: {err}"))?;
    Ok(RuntimeVal::Bool(true))
}

fn stderr_writeln32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 1, "stderr_writeln()")?;
    let data = runtime_display_arg(
        args.get(0).expect("checked arity"),
        runtime.heap(),
        "stderr_writeln data",
    )?;
    writeln!(std::io::stderr(), "{data}").map_err(|err| anyhow!("stderr write error: {err}"))?;
    Ok(RuntimeVal::Bool(true))
}

fn stderr_flush32(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "stderr_flush()")?;
    std::io::stderr()
        .flush()
        .map_err(|err| anyhow!("stderr flush error: {err}"))?;
    Ok(RuntimeVal::Bool(true))
}

fn mod_read32(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
    expect_arity(args, 0, "io.read()")?;
    stdin_read_all32(args, runtime)
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::vm::{NativeFunction32, RuntimeModuleState32};

    fn io_native(name: &str) -> Result<(u16, NativeFunction32)> {
        crate::runtime_native::runtime_native_export(&IoModule::new(), name)
    }

    fn call(name: &str, args: &[RuntimeVal]) -> Result<RuntimeVal> {
        let (_, function) = io_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        function(NativeArgs32::new(args), &mut runtime)
    }

    #[test]
    fn io_exports_use_runtime_native32() -> Result<()> {
        for name in [
            "read",
            "stdin_read",
            "stdin_read_line",
            "stdin_read_all",
            "stdin_flush",
            "stdout_write",
            "stdout_writeln",
            "stdout_flush",
            "stderr_write",
            "stderr_writeln",
            "stderr_flush",
        ] {
            let (_, function) = io_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
        }
        assert_eq!(io_native("stdin_read")?.0, lk_core::vm::NativeEntry32::VARIADIC);
        Ok(())
    }

    #[test]
    fn flush_functions_return_true() -> Result<()> {
        assert_eq!(call("stdin_flush", &[])?, RuntimeVal::Bool(true));
        assert_eq!(call("stdout_flush", &[])?, RuntimeVal::Bool(true));
        assert_eq!(call("stderr_flush", &[])?, RuntimeVal::Bool(true));
        Ok(())
    }

    #[test]
    fn write_functions_accept_runtime_values() -> Result<()> {
        assert_eq!(call("stdout_write", &[RuntimeVal::Int(0)])?, RuntimeVal::Bool(true));
        assert_eq!(call("stderr_write", &[RuntimeVal::Int(0)])?, RuntimeVal::Bool(true));
        Ok(())
    }
}
