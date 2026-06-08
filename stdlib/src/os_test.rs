#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{os::OsModule, register_stdlib_modules, runtime_native::runtime_string_value};
    use anyhow::{Result, anyhow};
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal, TypedList},
        vm::{NativeArgs, NativeEntry, NativeFunction, NativeRuntime, ProgramResult, RuntimeModuleState, VmContext},
    };

    fn run(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn os_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&OsModule::new(), name)
    }

    fn call_os(name: &str, args: &[RuntimeVal]) -> Result<(RuntimeVal, HeapStore)> {
        let (_, function) = os_native(name)?;
        call_plain(function, args)
    }

    fn call_plain(function: NativeFunction, args: &[RuntimeVal]) -> Result<(RuntimeVal, HeapStore)> {
        let NativeFunction::Plain(function) = function else {
            return Err(anyhow!("os function must use plain RuntimeNative"));
        };
        let mut state = RuntimeModuleState::default();
        let result = {
            let mut runtime = NativeRuntime::new(&mut state, None, None);
            function(NativeArgs::new(args), &mut runtime)?
        };
        Ok((result, state.into_heap()))
    }

    fn call_with_strings(name: &str, strings: &[&str]) -> Result<(RuntimeVal, HeapStore)> {
        let (_, function) = os_native(name)?;
        let NativeFunction::Plain(function) = function else {
            return Err(anyhow!("{name} must use plain RuntimeNative"));
        };
        let mut state = RuntimeModuleState::default();
        let args = strings
            .iter()
            .map(|value| runtime_string_value(value, state.heap_mut()))
            .collect::<Vec<_>>();
        let result = {
            let mut runtime = NativeRuntime::new(&mut state, None, None);
            function(NativeArgs::new(&args), &mut runtime)?
        };
        Ok((result, state.into_heap()))
    }

    fn runtime_str<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> Option<&'a str> {
        match value {
            RuntimeVal::ShortStr(value) => Some(value.as_str()),
            RuntimeVal::Obj(handle) => match heap.get(*handle) {
                Some(HeapValue::String(value)) => Some(value.as_ref()),
                _ => None,
            },
            _ => None,
        }
    }

    fn runtime_list<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> &'a TypedList {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected list object");
        };
        let Some(HeapValue::List(values)) = heap.get(*handle) else {
            panic!("expected list heap value");
        };
        values
    }

    #[test]
    fn test_os_arch_and_os_execute() -> Result<()> {
        let arch = run("use os; return os.arch();")?;
        assert_eq!(
            runtime_str(arch.first_return(), arch.state.heap()),
            Some(std::env::consts::ARCH)
        );
        let os = run("use os; return os.os();")?;
        assert_eq!(
            runtime_str(os.first_return(), os.state.heap()),
            Some(std::env::consts::OS)
        );
        Ok(())
    }

    #[test]
    fn test_os_exports_use_runtime_native_abi() -> Result<()> {
        for name in ["hostname", "arch", "os", "exit", "exec", "clock", "time", "epoch"] {
            let (_, function) = os_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
        }
        for name in ["env_get", "env_set", "env_unset", "dir_list", "dir_temp", "dir_current"] {
            let (_, function) = os_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
        }
        assert_eq!(os_native("exec")?.0, NativeEntry::VARIADIC);
        assert_eq!(os_native("env_get")?.0, NativeEntry::VARIADIC);
        Ok(())
    }

    #[test]
    fn test_os_env_get_default_and_mutation_errors() -> Result<()> {
        let var = "LK_TEST_ENV_SHOULD_NOT_EXIST_42";
        let src_default = format!("use os; return os.env_get(\"{}\", \"dflt\");", var);
        let default = run(&src_default)?;
        assert_eq!(runtime_str(default.first_return(), default.state.heap()), Some("dflt"));

        let src_set = format!("use os; return os.env_set(\"{}\", \"X\");", var);
        let err = run(&src_set).expect_err("env.set should be disabled");
        assert!(err.to_string().contains("disabled"));
        let src_unset = format!("use os; return os.env_unset(\"{}\");", var);
        let err = run(&src_unset).expect_err("env.unset should be disabled");
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

        let out = run("use os; return os.dir_temp();")?;
        if !matches!(out.first_return(), RuntimeVal::Nil) {
            assert!(
                runtime_str(out.first_return(), out.state.heap()).is_some(),
                "expected string or nil, got {:?}",
                out.first_return()
            );
        }
        let out = run("use os; return os.dir_current();")?;
        if !matches!(out.first_return(), RuntimeVal::Nil) {
            assert!(
                runtime_str(out.first_return(), out.state.heap()).is_some(),
                "expected string or nil, got {:?}",
                out.first_return()
            );
        }

        let src = format!("use os; return os.dir_list(\"{}\");", td.to_string_lossy());
        let out = run(&src)?;
        let TypedList::String(names) = runtime_list(out.first_return(), out.state.heap()) else {
            panic!("expected typed string list");
        };
        assert!(names.iter().any(|name| name.as_ref() == "a.txt"));
        assert!(names.iter().any(|name| name.as_ref() == "b.txt"));

        let _ = std::fs::remove_file(f1);
        let _ = std::fs::remove_file(f2);
        let _ = std::fs::remove_dir_all(PathBuf::from(&td));
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_os_exec_capture_unix() -> Result<()> {
        let out = run("use os; return os.exec(\"/bin/echo\", [\"hello\"]);")?;
        assert_eq!(
            runtime_str(out.first_return(), out.state.heap()).map(str::trim_end),
            Some("hello")
        );
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_os_exec_stream_mode_returns_line_list_unix() -> Result<()> {
        let out = run("use os; return os.exec(\"/bin/echo\", [\"a\", \"b\"], true);")?;
        let TypedList::String(list) = runtime_list(out.first_return(), out.state.heap()) else {
            panic!("stream mode should return typed string list");
        };
        assert_eq!(list.as_slice(), &[Arc::<str>::from("a b")]);
        Ok(())
    }

    #[test]
    #[cfg(unix)]
    fn test_os_direct_runtime_calls() -> Result<()> {
        let (arch, heap) = call_os("arch", &[])?;
        assert_eq!(runtime_str(&arch, &heap), Some(std::env::consts::ARCH));
        assert!(matches!(call_os("time", &[])?.0, RuntimeVal::Int(_)));
        let (output, heap) = call_with_strings("exec", &["/bin/echo"])?;
        assert_eq!(runtime_str(&output, &heap).map(str::trim_end), Some(""));
        Ok(())
    }
}
