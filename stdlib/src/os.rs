use anyhow::Result;
use lkr_core::{module::Module, val::Val, vm::VmContext};
use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone)]
struct EnvObject;

impl EnvObject {
    fn create() -> Val {
        let mut methods = HashMap::new();
        methods.insert("get".to_string(), Val::RustFunction(Self::get));
        methods.insert("set".to_string(), Val::RustFunction(Self::set));
        methods.insert("unset".to_string(), Val::RustFunction(Self::unset));
        methods.into()
    }

    fn get(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 && args.len() != 2 {
            return Err(anyhow::anyhow!(
                "env.get() takes 1 or 2 arguments: variable_name [, default_value]"
            ));
        }

        let var_name = match &args[0] {
            Val::Str(name) => &**name,
            _ => return Err(anyhow::anyhow!("first argument must be a string")),
        };

        // Get default value if provided
        let default_val = if args.len() == 2 {
            match &args[1] {
                Val::Str(val) => Some(&**val),
                Val::Nil => None,
                _ => return Err(anyhow::anyhow!("second argument must be a string or nil")),
            }
        } else {
            None
        };

        match std::env::var_os(var_name) {
            Some(value) => match value.into_string() {
                Ok(value_str) => Ok(Val::Str(value_str.into())),
                Err(_) => Ok(Val::Nil),
            },
            None => match default_val {
                Some(default) => Ok(Val::Str(default.into())),
                None => Ok(Val::Nil),
            },
        }
    }

    fn set(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 2 {
            return Err(anyhow::anyhow!(
                "env.set() takes exactly 2 arguments: variable_name, value"
            ));
        }

        let var_name = match &args[0] {
            Val::Str(name) => &**name,
            _ => return Err(anyhow::anyhow!("first argument must be a string")),
        };

        let value = match &args[1] {
            Val::Str(val) => &**val,
            _ => return Err(anyhow::anyhow!("second argument must be a string")),
        };

        unsafe {
            std::env::set_var(var_name, value);
        }
        Ok(Val::Bool(true))
    }

    fn unset(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("env.unset() takes exactly 1 argument: variable_name"));
        }

        let var_name = match &args[0] {
            Val::Str(name) => &**name,
            _ => return Err(anyhow::anyhow!("argument must be a string")),
        };

        unsafe {
            std::env::remove_var(var_name);
        }
        Ok(Val::Bool(true))
    }
}

struct DirObject;

impl DirObject {
    fn create() -> Val {
        let mut methods = HashMap::new();
        methods.insert("list".to_string(), Val::RustFunction(Self::list));
        methods.insert("temp".to_string(), Val::RustFunction(Self::temp_dir));
        methods.insert("current".to_string(), Val::RustFunction(Self::current_dir));
        methods.into()
    }

    fn list(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() != 1 {
            return Err(anyhow::anyhow!("dir.list() takes exactly 1 argument: path"));
        }

        let path = match &args[0] {
            Val::Str(p) => &**p,
            _ => return Err(anyhow::anyhow!("argument must be a string")),
        };

        let mut entries = Vec::new();
        match std::fs::read_dir(path) {
            Ok(read_dir) => {
                for entry in read_dir {
                    match entry {
                        Ok(dir_entry) => {
                            if let Some(name) = dir_entry.file_name().to_str() {
                                entries.push(Val::Str(name.into()))
                            }
                        }
                        Err(_) => continue,
                    }
                }
                Ok(Val::List(Arc::from(entries)))
            }
            Err(e) => Err(anyhow::anyhow!("failed to read directory: {}", e)),
        }
    }

    fn temp_dir(_args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        Ok(match std::env::temp_dir().into_os_string().into_string() {
            Ok(path) => Val::Str(path.into()),
            Err(_) => Val::Nil,
        })
    }

    fn current_dir(_args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        Ok(match std::env::current_dir() {
            Ok(path) => match path.into_os_string().into_string() {
                Ok(path_str) => Val::Str(path_str.into()),
                Err(_) => Val::Nil,
            },
            Err(_) => Val::Nil,
        })
    }
}

#[derive(Debug)]
pub struct OsModule {
    functions: HashMap<String, Val>,
}

impl Default for OsModule {
    fn default() -> Self {
        Self::new()
    }
}

impl OsModule {
    pub fn new() -> Self {
        let mut functions = HashMap::new();

        // Register os functions as Rust functions
        functions.insert("hostname".to_string(), Val::RustFunction(Self::hostname));
        functions.insert("arch".to_string(), Val::RustFunction(Self::arch));
        functions.insert("os".to_string(), Val::RustFunction(Self::os));
        functions.insert("exit".to_string(), Val::RustFunction(Self::exit));
        functions.insert("exec".to_string(), Val::RustFunction(Self::exec));

        // Add env object
        functions.insert("env".to_string(), EnvObject::create());

        // Add dir object
        functions.insert("dir".to_string(), DirObject::create());

        Self { functions }
    }

    /// Get system hostname
    fn hostname(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if !args.is_empty() {
            return Err(anyhow::anyhow!("hostname() takes no arguments"));
        }

        match std::env::var_os("HOSTNAME") {
            Some(hostname) => match hostname.into_string() {
                Ok(hostname_str) => Ok(Val::Str(hostname_str.into())),
                Err(_) => Ok(Val::Str("localhost".into())),
            },
            None => match std::env::var_os("COMPUTERNAME") {
                Some(hostname) => match hostname.into_string() {
                    Ok(hostname_str) => Ok(Val::Str(hostname_str.into())),
                    Err(_) => Ok(Val::Str("localhost".into())),
                },
                None => Ok(Val::Str("localhost".into())),
            },
        }
    }

    /// Get system architecture
    fn arch(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if !args.is_empty() {
            return Err(anyhow::anyhow!("arch() takes no arguments"));
        }

        Ok(Val::Str(std::env::consts::ARCH.into()))
    }

    /// Get operating system
    fn os(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if !args.is_empty() {
            return Err(anyhow::anyhow!("os() takes no arguments"));
        }

        Ok(Val::Str(std::env::consts::OS.into()))
    }

    /// Exit the program
    fn exit(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
        if args.len() > 1 {
            return Err(anyhow::anyhow!("exit() takes at most 1 argument: exit_code"));
        }

        let exit_code = if args.is_empty() {
            0
        } else {
            match &args[0] {
                Val::Int(code) => *code as i32,
                _ => return Err(anyhow::anyhow!("exit code must be an integer")),
            }
        };

        std::process::exit(exit_code);
    }

    /// Execute an external command.
    /// Usage:
    /// - os.exec(cmd: String) -> String (stdout captured)
    /// - os.exec(cmd: String, args: List<String>) -> String
    /// - os.exec(cmd: String, stream: Bool) -> Stream (when true)
    /// - os.exec(cmd: String, args: List<String>, stream: Bool) -> Stream
    fn exec(args: &[Val], ctx: &mut VmContext) -> Result<Val> {
        use anyhow::anyhow;
        use std::process::{Command, Stdio};

        if args.is_empty() || args.len() > 3 {
            return Err(anyhow!("exec() expects 1-3 arguments: cmd[, args_list][, stream_bool]"));
        }

        // cmd
        let cmd = match &args[0] {
            Val::Str(s) => s.as_ref(),
            _ => return Err(anyhow!("first argument (cmd) must be a string")),
        };

        // parse args list and stream flag
        let mut argv: Vec<String> = Vec::new();
        let mut stream = false;

        if args.len() >= 2 {
            match &args[1] {
                Val::List(list) => {
                    for v in list.iter() {
                        match v {
                            Val::Str(s) => argv.push(s.to_string()),
                            other => argv.push(other.to_string()),
                        }
                    }
                }
                Val::Bool(b) => {
                    stream = *b;
                }
                _ => return Err(anyhow!("second argument must be args list or stream bool")),
            }
        }

        if args.len() == 3 {
            match &args[2] {
                Val::Bool(b) => stream = *b,
                _ => return Err(anyhow!("third argument must be a boolean (stream)")),
            }
        }

        if stream {
            // Synchronously capture stdout, split into lines, and expose as a Stream
            let output = Command::new(cmd)
                .args(&argv)
                .stdout(Stdio::piped())
                .output()
                .map_err(|e| anyhow!("failed to execute '{}': {}", cmd, e))?;
            let s = match String::from_utf8(output.stdout) {
                Ok(s) => s,
                Err(_) => return Ok(Val::Nil),
            };
            let mut items: Vec<Val> = Vec::new();
            for mut line in s.lines().map(|x| x.to_string()) {
                if line.ends_with('\r') {
                    line.pop();
                }
                items.push(Val::Str(line.into()));
            }
            let list_val = Val::List(items.into());
            if let Some(to_stream) = lkr_core::val::methods::find_method_for_val(&list_val, "to_stream") {
                let res = to_stream(&[list_val], ctx)?;
                return Ok(res);
            }
            return Ok(list_val);
        }

        // Non-streaming: capture stdout fully
        let output = Command::new(cmd)
            .args(&argv)
            .output()
            .map_err(|e| anyhow!("failed to execute '{}': {}", cmd, e))?;
        match String::from_utf8(output.stdout) {
            Ok(s) => Ok(Val::Str(s.into())),
            Err(_) => Ok(Val::Nil),
        }
    }
}

impl Module for OsModule {
    fn name(&self) -> &str {
        "os"
    }

    fn description(&self) -> &str {
        "Operating system interface"
    }

    fn register(&self, _registry: &mut lkr_core::module::ModuleRegistry) -> Result<()> {
        Ok(())
    }

    fn exports(&self) -> HashMap<String, Val> {
        self.functions.clone()
    }
}
