#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{os::OsModule, register_stdlib_modules};
    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal},
        vm::{NativeFunction, ProgramResult, VmContext},
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
        for name in ["hostname", "arch", "os", "clock", "time", "epoch"] {
            let (_, function) = os_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
        }
        for removed in ["exec", "env_get", "dir_list", "path_join", "path_sep"] {
            assert!(os_native(removed).is_err(), "{removed} should move out of os");
        }
        Ok(())
    }
}
