#[cfg(test)]
mod tests {
    use crate::tcp::TcpModule;
    use lkr_core::{module::Module, stmt::ModuleResolver, val::Val, vm::VmContext};
    use std::sync::Arc;

    fn create_test_env() -> VmContext {
        VmContext::new().with_resolver(Arc::new(ModuleResolver::new()))
    }

    #[test]
    fn test_tcp_module_creation() {
        let tcp_module = TcpModule::new();
        assert_eq!(tcp_module.name(), "tcp");

        let exports = tcp_module.exports();
        assert!(exports.contains_key("connect"));
        assert!(exports.contains_key("bind"));
        assert!(exports.contains_key("close"));
        assert!(exports.contains_key("read"));
        assert!(exports.contains_key("write"));
        assert!(exports.contains_key("accept"));
    }

    #[test]
    fn test_tcp_connect_requires_args() {
        let tcp_module = TcpModule::new();
        let exports = tcp_module.exports();
        let connect_fn = exports.get("connect").unwrap();
        let mut env = create_test_env();

        // Test with no arguments
        let result = match connect_fn {
            Val::RustFunction(f) => f(&[], &mut env),
            _ => panic!("Expected RustFunction"),
        };

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 2 arguments"));
    }

    #[test]
    fn test_tcp_bind_requires_args() {
        let tcp_module = TcpModule::new();
        let exports = tcp_module.exports();
        let bind_fn = exports.get("bind").unwrap();
        let mut env = create_test_env();

        // Test with no arguments
        let result = match bind_fn {
            Val::RustFunction(f) => f(&[], &mut env),
            _ => panic!("Expected RustFunction"),
        };

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 2 arguments"));
    }

    #[test]
    fn test_tcp_close_requires_args() {
        let tcp_module = TcpModule::new();
        let exports = tcp_module.exports();
        let close_fn = exports.get("close").unwrap();
        let mut env = create_test_env();

        // Test with no arguments
        let result = match close_fn {
            Val::RustFunction(f) => f(&[], &mut env),
            _ => panic!("Expected RustFunction"),
        };

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("requires 1 argument"));
    }
}
