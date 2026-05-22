#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{os::OsModule, register_stdlib_modules, runtime_native::runtime_string_value};
    use anyhow::{Result, anyhow};
    use lk_core::{
        module::{Module, ModuleRegistry},
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{CallableValue, HeapStore, HeapValue, RuntimeVal, Val, runtime_val_to_val},
        vm::{NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, RuntimeModuleState32, VmContext},
    };

    fn run32(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute32_with_ctx(&mut env)
    }

    fn os_native(name: &str) -> Result<(u16, NativeFunction32)> {
        let exports = OsModule::new().exports();
        runtime_native_from_val(exports.get(name).ok_or_else(|| anyhow!("{name} export present"))?, name)
    }

    fn runtime_native_from_val(value: &Val, name: &str) -> Result<(u16, NativeFunction32)> {
        let Val::Obj(object) = value else {
            return Err(anyhow!("{name} must be a heap callable"));
        };
        let HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) = object.as_ref() else {
            return Err(anyhow!("{name} must be RuntimeNative32"));
        };
        Ok((*arity, function.clone()))
    }

    fn call_os(name: &str, args: &[RuntimeVal]) -> Result<Val> {
        let (_, function) = os_native(name)?;
        call_plain(function, args)
    }

    fn call_plain(function: NativeFunction32, args: &[RuntimeVal]) -> Result<Val> {
        let NativeFunction32::Plain(function) = function else {
            return Err(anyhow!("os function must use plain RuntimeNative32"));
        };
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let mut runtime = NativeRuntime32 {
            state: &mut state,
            ctx: None,
            module: None,
        };
        let result = function(NativeArgs32::new(args), &mut runtime)?;
        runtime_val_to_val(&result, &runtime.state.heap)
    }

    fn call_with_strings(name: &str, strings: &[&str]) -> Result<Val> {
        let (_, function) = os_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            return Err(anyhow!("{name} must use plain RuntimeNative32"));
        };
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let args = strings
            .iter()
            .map(|value| runtime_string_value(value, &mut state.heap))
            .collect::<Vec<_>>();
        let mut runtime = NativeRuntime32 {
            state: &mut state,
            ctx: None,
            module: None,
        };
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        runtime_val_to_val(&result, &runtime.state.heap)
    }

    #[test]
    fn test_os_arch_and_os_execute32() -> Result<()> {
        assert_eq!(
            run32("import os; return os.arch();")?.as_str(),
            Some(std::env::consts::ARCH)
        );
        assert_eq!(
            run32("import os; return os.os();")?.as_str(),
            Some(std::env::consts::OS)
        );
        Ok(())
    }

    #[test]
    fn test_os_exports_use_runtime_native32_abi() -> Result<()> {
        for name in ["hostname", "arch", "os", "exit", "exec", "clock", "time", "epoch"] {
            let (_, function) = os_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
        }
        for name in ["env_get", "env_set", "env_unset", "dir_list", "dir_temp", "dir_current"] {
            let (_, function) = os_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
        }
        assert_eq!(os_native("exec")?.0, NativeEntry32::VARIADIC);
        assert_eq!(os_native("env_get")?.0, NativeEntry32::VARIADIC);
        Ok(())
    }

    #[test]
    fn test_os_env_get_default_and_mutation_errors() -> Result<()> {
        let var = "LK_TEST_ENV_SHOULD_NOT_EXIST_42";
        let src_default = format!("import os; return os.env_get(\"{}\", \"dflt\");", var);
        assert_eq!(run32(&src_default)?, Val::from_str("dflt"));

        let src_set = format!("import os; return os.env_set(\"{}\", \"X\");", var);
        let err = run32(&src_set).expect_err("env.set should be disabled");
        assert!(err.to_string().contains("disabled"));
        let src_unset = format!("import os; return os.env_unset(\"{}\");", var);
        let err = run32(&src_unset).expect_err("env.unset should be disabled");
        assert!(err.to_string().contains("disabled"));
        Ok(())
    }

    #[test]
    fn test_os_dir_temp_current_and_list() -> Result<()> {
        use std::fs::{File, create_dir_all};
        use std::io::Write;
        use std::path::PathBuf;

        let mut td = std::env::temp_dir();
        td.push("lk_os_test");
        td.push(format!("case_{}", std::process::id()));
        create_dir_all(&td)?;

        let mut f1 = td.clone();
        f1.push("a.txt");
        let mut f2 = td.clone();
        f2.push("b.txt");
        writeln!(File::create(&f1)?, "hello")?;
        writeln!(File::create(&f2)?, "world")?;

        let out = run32("import os; return os.dir_temp();")?;
        if !matches!(out, Val::Nil) {
            assert!(out.as_str().is_some(), "expected string or nil, got {out:?}");
        }
        let out = run32("import os; return os.dir_current();")?;
        if !matches!(out, Val::Nil) {
            assert!(out.as_str().is_some(), "expected string or nil, got {out:?}");
        }

        let src = format!("import os; return os.dir_list(\"{}\");", td.to_string_lossy());
        let out = run32(&src)?;
        let list = out.as_list().expect("expected List");
        let names = list.iter().filter_map(|v| v.as_str()).collect::<Vec<_>>();
        assert!(names.contains(&"a.txt"));
        assert!(names.contains(&"b.txt"));

        let _ = std::fs::remove_file(f1);
        let _ = std::fs::remove_file(f2);
        let _ = std::fs::remove_dir_all(PathBuf::from(&td));
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_os_exec_capture_unix() -> Result<()> {
        let out = run32("import os; return os.exec(\"/bin/echo\", [\"hello\"]);")?;
        assert_eq!(out.as_str().map(str::trim_end), Some("hello"));
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_os_exec_stream_mode_returns_line_list_unix() -> Result<()> {
        let out = run32("import os; return os.exec(\"/bin/echo\", [\"a\", \"b\"], true);")?;
        let list = out.as_list().expect("stream mode currently returns line list");
        assert_eq!(list.as_slice(), &[Val::from_str("a b")]);
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_os_direct_runtime_calls() -> Result<()> {
        assert_eq!(call_os("arch", &[])?.as_str(), Some(std::env::consts::ARCH));
        assert!(matches!(call_os("time", &[])?, Val::Int(_)));
        assert_eq!(
            call_with_strings("exec", &["/bin/echo"])?.as_str().map(str::trim_end),
            Some("")
        );
        Ok(())
    }
}
