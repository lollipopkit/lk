use anyhow::Result;
use lkr_core::module::Module;
use lkr_core::val::Val;
use lkr_core::vm::VmContext;
use std::collections::HashMap;
use std::io::{BufRead, Read, Write};

fn make_stdin_object() -> Val {
    let mut methods = HashMap::new();
    methods.insert("read".to_string(), Val::RustFunction(stdin_read));
    methods.insert("read_line".to_string(), Val::RustFunction(stdin_read_line));
    methods.insert("read_all".to_string(), Val::RustFunction(stdin_read_all));
    // stdin flush is a no-op for convenience; returns true
    methods.insert("flush".to_string(), Val::RustFunction(stdin_flush));
    methods.into()
}

fn make_stdout_object() -> Val {
    let mut methods = HashMap::new();
    methods.insert("write".to_string(), Val::RustFunction(stdout_write));
    methods.insert("writeln".to_string(), Val::RustFunction(stdout_writeln));
    methods.insert("flush".to_string(), Val::RustFunction(stdout_flush));
    methods.into()
}

fn make_stderr_object() -> Val {
    let mut methods = HashMap::new();
    methods.insert("write".to_string(), Val::RustFunction(stderr_write));
    methods.insert("writeln".to_string(), Val::RustFunction(stderr_writeln));
    methods.insert("flush".to_string(), Val::RustFunction(stderr_flush));
    methods.into()
}

fn stdin_read(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() > 1 {
        return Err(anyhow::anyhow!("stdin.read() takes at most 1 argument: [bytes]"));
    }

    let mut handle = std::io::stdin().lock();
    if args.is_empty() {
        // default: read a single line
        let mut line = String::new();
        match handle.read_line(&mut line) {
            Ok(0) => Ok(Val::Nil), // EOF
            Ok(_) => {
                if line.ends_with('\n') {
                    line.pop();
                    if line.ends_with('\r') {
                        line.pop();
                    }
                }
                Ok(Val::Str(line.into()))
            }
            Err(e) => Err(anyhow::anyhow!("stdin read error: {}", e)),
        }
    } else {
        let n = match &args[0] {
            Val::Int(i) if *i >= 0 => *i as usize,
            _ => return Err(anyhow::anyhow!("bytes must be a non-negative integer")),
        };
        if n == 0 {
            return Ok(Val::Str("".into()));
        }
        let mut buf = vec![0u8; n];
        match handle.read(&mut buf) {
            Ok(0) => Ok(Val::Nil),
            Ok(read) => {
                buf.truncate(read);
                match String::from_utf8(buf) {
                    Ok(s) => Ok(Val::Str(s.into())),
                    Err(_) => Ok(Val::Nil),
                }
            }
            Err(e) => Err(anyhow::anyhow!("stdin read error: {}", e)),
        }
    }
}

fn stdin_read_line(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
    if !args.is_empty() {
        return Err(anyhow::anyhow!("stdin.read_line() takes no arguments"));
    }
    stdin_read(&[], ctx)
}

fn stdin_flush(_args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    // No-op; included for API symmetry. Return true for convenience.
    Ok(Val::Bool(true))
}

fn stdin_read_all(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if !args.is_empty() {
        return Err(anyhow::anyhow!("stdin.read_all() takes no arguments"));
    }
    let mut s = String::new();
    let res = std::io::stdin().lock().read_to_string(&mut s);
    match res {
        Ok(_) => Ok(Val::Str(s.into())),
        Err(e) => Err(anyhow::anyhow!("stdin read error: {}", e)),
    }
}

fn stdout_write(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow::anyhow!("stdout.write() requires 1 argument: data"));
    }
    let data = match &args[0] {
        Val::Str(s) => s.as_ref(),
        v => &v.to_string(),
    };
    match std::io::stdout().write_all(data.as_bytes()) {
        Ok(()) => Ok(Val::Bool(true)),
        Err(e) => Err(anyhow::anyhow!("stdout write error: {}", e)),
    }
}

fn stdout_writeln(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow::anyhow!("stdout.writeln() requires 1 argument: data"));
    }
    let data = match &args[0] {
        Val::Str(s) => s.as_ref(),
        v => &v.to_string(),
    };
    match writeln!(std::io::stdout(), "{}", data) {
        Ok(()) => Ok(Val::Bool(true)),
        Err(e) => Err(anyhow::anyhow!("stdout write error: {}", e)),
    }
}

fn stdout_flush(_args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    match std::io::stdout().flush() {
        Ok(()) => Ok(Val::Bool(true)),
        Err(e) => Err(anyhow::anyhow!("stdout flush error: {}", e)),
    }
}

fn stderr_write(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow::anyhow!("stderr.write() requires 1 argument: data"));
    }
    let data = match &args[0] {
        Val::Str(s) => s.as_ref(),
        v => &v.to_string(),
    };
    match std::io::stderr().write_all(data.as_bytes()) {
        Ok(()) => Ok(Val::Bool(true)),
        Err(e) => Err(anyhow::anyhow!("stderr write error: {}", e)),
    }
}

fn stderr_writeln(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    if args.len() != 1 {
        return Err(anyhow::anyhow!("stderr.writeln() requires 1 argument: data"));
    }
    let data = match &args[0] {
        Val::Str(s) => s.as_ref(),
        v => &v.to_string(),
    };
    match writeln!(std::io::stderr(), "{}", data) {
        Ok(()) => Ok(Val::Bool(true)),
        Err(e) => Err(anyhow::anyhow!("stderr write error: {}", e)),
    }
}

fn stderr_flush(_args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    match std::io::stderr().flush() {
        Ok(()) => Ok(Val::Bool(true)),
        Err(e) => Err(anyhow::anyhow!("stderr flush error: {}", e)),
    }
}

#[derive(Debug)]
pub struct IoModule {
    functions: HashMap<String, Val>,
}

impl Default for IoModule {
    fn default() -> Self {
        Self::new()
    }
}

impl IoModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        // Create objects for stdin, stdout, stderr
        functions.insert("stdin".to_string(), make_stdin_object());
        functions.insert("stdout".to_string(), make_stdout_object());
        functions.insert("stderr".to_string(), make_stderr_object());

        // Convenience top-level helpers
        functions.insert("read".to_string(), Val::RustFunction(mod_read));

        IoModule { functions }
    }
}

impl Module for IoModule {
    fn name(&self) -> &str {
        "io"
    }

    fn register(&self, _registry: &mut lkr_core::module::ModuleRegistry) -> Result<()> {
        // Don't register functions globally - they should be accessed via module.function()
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}

// ----- Top-level helpers -----

fn read_all_to_string() -> anyhow::Result<String> {
    let mut s = String::new();
    std::io::stdin().lock().read_to_string(&mut s)?;
    Ok(s)
}

fn mod_read(args: &[Val], _ctx: &mut VmContext) -> anyhow::Result<Val> {
    if !args.is_empty() {
        return Err(anyhow::anyhow!("io.read() takes no arguments"));
    }
    Ok(Val::Str(read_all_to_string()?.into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_io_module_has_objects() -> Result<()> {
        let module = IoModule::new();
        let exports = module.exports();
        assert!(exports.contains_key("stdin"));
        assert!(exports.contains_key("stdout"));
        assert!(exports.contains_key("stderr"));
        Ok(())
    }

    #[test]
    fn test_stdin_flush_returns_true() -> Result<()> {
        let mut env = VmContext::new();
        let result = stdin_flush(&[], &mut env)?;
        assert_eq!(result, Val::Bool(true));
        Ok(())
    }

    #[test]
    fn test_stdout_flush_returns_true() -> Result<()> {
        let mut env = VmContext::new();
        let result = stdout_flush(&[], &mut env)?;
        assert_eq!(result, Val::Bool(true));
        Ok(())
    }

    #[test]
    fn test_stderr_flush_returns_true() -> Result<()> {
        let mut env = VmContext::new();
        let result = stderr_flush(&[], &mut env)?;
        assert_eq!(result, Val::Bool(true));
        Ok(())
    }
}
