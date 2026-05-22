#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{math::MathModule, register_stdlib_modules};
    use anyhow::{Result, anyhow};
    use lk_core::{
        module::{Module, ModuleRegistry},
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{CallableValue, HeapStore, HeapValue, RuntimeVal, Val, runtime_val_to_val},
        vm::{NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, RuntimeModuleState32, VmContext},
    };

    fn execute_math32(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute32_with_ctx(&mut env)
    }

    fn math_native(name: &str) -> Result<(u16, NativeFunction32)> {
        let exports = MathModule::new().exports();
        let value = exports.get(name).ok_or_else(|| anyhow!("{name} export present"))?;
        let Val::Obj(object) = value else {
            return Err(anyhow!("{name} must be a heap callable"));
        };
        let HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) = object.as_ref() else {
            return Err(anyhow!("{name} must be RuntimeNative32"));
        };
        Ok((*arity, function.clone()))
    }

    fn call_math(name: &str, args: &[RuntimeVal], named: &[(String, RuntimeVal)]) -> Result<RuntimeVal> {
        let (_, function) = math_native(name)?;
        let NativeFunction32::Plain(function) = function else {
            return Err(anyhow!("{name} must use plain RuntimeNative32"));
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
        function(NativeArgs32::new_with_named(args, named), &mut runtime)
    }

    #[test]
    fn test_math_abs_positive() -> Result<()> {
        assert_eq!(execute_math32("import math; return math.abs(42);")?, Val::Int(42));
        Ok(())
    }

    #[test]
    fn test_math_abs_negative() -> Result<()> {
        assert_eq!(execute_math32("import math; return math.abs(-42);")?, Val::Int(42));
        Ok(())
    }

    #[test]
    fn test_math_sqrt() -> Result<()> {
        assert_eq!(execute_math32("import math; return math.sqrt(16);")?, Val::Float(4.0));
        Ok(())
    }

    #[test]
    fn test_math_constants() -> Result<()> {
        let result = execute_math32("import math; return math.pi;")?;
        let Val::Float(value) = result else {
            panic!("Expected float result");
        };
        assert!((value - std::f64::consts::PI).abs() < 1e-10);
        Ok(())
    }

    #[test]
    fn test_math_clamp_named_arguments() -> Result<()> {
        let source = r#"
            import math;
            let a = math.clamp(150);
            let b = math.clamp(-5);
            let c = math.clamp(5, min: 2, max: 4);
            let d = math.clamp(5, 0, 3);
            return [a, b, c, d];
        "#;
        let result = execute_math32(source)?;
        let expected = Val::list(vec![Val::Int(100), Val::Int(0), Val::Int(4), Val::Int(3)].into());
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_math_clamp_duplicate_named_argument_error() {
        let named_args = vec![
            ("min".to_string(), RuntimeVal::Int(0)),
            ("min".to_string(), RuntimeVal::Int(1)),
        ];
        let err =
            call_math("clamp", &[RuntimeVal::Int(5)], &named_args).expect_err("duplicate named arguments should error");
        assert!(err.to_string().contains("duplicate named argument"));
    }

    #[test]
    fn test_math_sqrt_negative_error() {
        let err = call_math("sqrt", &[RuntimeVal::Int(-1)], &[]).expect_err("negative input should fail");
        assert!(err.to_string().contains("must be non-negative"));
    }

    #[test]
    fn test_math_log_non_positive_error() {
        let err = call_math("log", &[RuntimeVal::Int(0)], &[]).expect_err("non-positive input should fail");
        assert!(err.to_string().contains("must be positive"));
    }

    #[test]
    fn test_math_atan2_mixed_numeric_types() -> Result<()> {
        let result = call_math("atan2", &[RuntimeVal::Int(1), RuntimeVal::Float(0.0)], &[])?;
        let RuntimeVal::Float(angle) = result else {
            panic!("atan2 should return float");
        };
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-10);
        Ok(())
    }

    #[test]
    fn test_math_selected_functions_use_runtime_native32_abi() -> Result<()> {
        for name in [
            "abs", "sqrt", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "log", "log10", "log2", "exp", "pow",
            "floor", "ceil", "round", "min", "max", "random",
        ] {
            let (arity, function) = math_native(name)?;
            assert!(
                matches!(function, NativeFunction32::Plain(_)),
                "{name} should use plain RuntimeNative32"
            );
            assert_ne!(
                arity,
                NativeEntry32::VARIADIC,
                "{name} should have fixed positional arity"
            );
        }

        let (arity, function) = math_native("clamp")?;
        assert!(matches!(function, NativeFunction32::Plain(_)));
        assert_eq!(arity, NativeEntry32::VARIADIC);
        Ok(())
    }

    #[test]
    fn test_math_runtime_result_converts_to_legacy_val_for_boundary() -> Result<()> {
        let result = call_math("min", &[RuntimeVal::Float(2.5), RuntimeVal::Int(3)], &[])?;
        let heap = HeapStore::new();
        let value = runtime_val_to_val(&result, &heap)?;
        assert_eq!(value, Val::Float(2.5));
        Ok(())
    }
}
