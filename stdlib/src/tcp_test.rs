#[cfg(test)]
mod tests {
    use crate::tcp::TcpModule;
    use anyhow::{Result, anyhow, bail};
    use lk_core::{
        module::Module,
        val::{CallableValue, HeapValue, RuntimeVal, Val},
        vm::{NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeModuleState32},
    };

    fn tcp_native(name: &str) -> Result<(u16, NativeFunction32)> {
        let exports = TcpModule::new().exports();
        let value = exports.get(name).ok_or_else(|| anyhow!("{name} export present"))?;
        let Val::Obj(object) = value else {
            bail!("{name} must be a heap callable");
        };
        let HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) = object.as_ref() else {
            bail!("{name} must be RuntimeNative32");
        };
        Ok((*arity, function.clone()))
    }

    fn call(name: &str, args: &[RuntimeVal]) -> Result<RuntimeVal> {
        let (_, function) = tcp_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            bail!("{name} must use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        function(NativeArgs32::new(args), &mut runtime)
    }

    #[test]
    fn test_tcp_module_creation() -> Result<()> {
        let tcp_module = TcpModule::new();
        assert_eq!(tcp_module.name(), "tcp");

        let exports = tcp_module.exports();
        for name in ["connect", "bind", "close", "read", "write", "accept"] {
            assert!(exports.contains_key(name));
            let (_, function) = tcp_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
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
