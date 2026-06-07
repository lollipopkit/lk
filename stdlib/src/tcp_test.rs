#[cfg(test)]
mod tests {
    use crate::tcp::TcpModule;
    use anyhow::{Result, bail};
    use lk_core::{
        module::ModuleProvider,
        val::RuntimeVal,
        vm::{NativeArgs, NativeFunction, NativeRuntime, RuntimeModuleState},
    };

    fn tcp_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&TcpModule::new(), name)
    }

    fn call(name: &str, args: &[RuntimeVal]) -> Result<RuntimeVal> {
        let (_, function) = tcp_native(name)?;
        let NativeFunction::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        function(NativeArgs::new(args), &mut runtime)
    }

    #[test]
    fn test_tcp_module_creation() -> Result<()> {
        let tcp_module = TcpModule::new();
        assert_eq!(tcp_module.name(), "tcp");

        for name in ["connect", "bind", "close", "read", "write", "accept"] {
            let (_, function) = tcp_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
        }
        Ok(())
    }

    #[test]
    fn test_tcp_connect_requires_args() {
        let err = call("connect", &[]).expect_err("connect arity should fail");
        assert!(err.to_string().contains("requires 2 arguments"));
    }

    #[test]
    fn test_tcp_bind_requires_args() {
        let err = call("bind", &[]).expect_err("bind arity should fail");
        assert!(err.to_string().contains("requires 2 arguments"));
    }

    #[test]
    fn test_tcp_close_requires_args() {
        let err = call("close", &[]).expect_err("close arity should fail");
        assert!(err.to_string().contains("requires 1 argument"));
    }
}
