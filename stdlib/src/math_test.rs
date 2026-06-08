#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{math::MathModule, register_stdlib_modules};
    use anyhow::{Result, anyhow};
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal, ShortStr, TypedList},
        vm::{NativeArgs, NativeEntry, NativeFunction, NativeRuntime, ProgramResult, RuntimeModuleState, VmContext},
    };

    fn execute_math(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn math_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&MathModule::new(), name)
    }

    fn call_math(name: &str, args: &[RuntimeVal]) -> Result<RuntimeVal> {
        call_math_named_stack(name, args, &[], 0)
    }

    fn call_math_named_stack(
        name: &str,
        args: &[RuntimeVal],
        named_stack: &[RuntimeVal],
        named_count: u16,
    ) -> Result<RuntimeVal> {
        let (_, function) = math_native(name)?;
        let NativeFunction::Plain(function) = function else {
            return Err(anyhow!("{name} must use plain RuntimeNative"));
        };
        let mut state = RuntimeModuleState::default();
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        let args = if named_count == 0 {
            NativeArgs::new(args)
        } else {
            NativeArgs::new_with_named_stack(args, named_stack, 0, named_count)
        };
        function(args, &mut runtime)
    }

    fn runtime_list<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> &'a TypedList {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(*handle) else {
            panic!("expected runtime list heap value");
        };
        list
    }

    #[test]
    fn test_math_abs_positive() -> Result<()> {
        assert_eq!(
            execute_math("use math; return math.abs(42);")?.first_return(),
            &RuntimeVal::Int(42)
        );
        Ok(())
    }

    #[test]
    fn test_math_abs_negative() -> Result<()> {
        assert_eq!(
            execute_math("use math; return math.abs(-42);")?.first_return(),
            &RuntimeVal::Int(42)
        );
        Ok(())
    }

    #[test]
    fn test_math_sqrt() -> Result<()> {
        assert_eq!(
            execute_math("use math; return math.sqrt(16);")?.first_return(),
            &RuntimeVal::Float(4.0)
        );
        Ok(())
    }

    #[test]
    fn test_math_constants() -> Result<()> {
        let result = execute_math("use math; return math.pi;")?;
        let RuntimeVal::Float(value) = result.first_return() else {
            panic!("Expected float result");
        };
        assert!((*value - std::f64::consts::PI).abs() < 1e-10);
        Ok(())
    }

    #[test]
    fn test_math_clamp_named_arguments() -> Result<()> {
        let source = r#"
            use math;
            let a = math.clamp(150);
            let b = math.clamp(-5);
            let c = math.clamp(5, min: 2, max: 4);
            let d = math.clamp(5, 0, 3);
            return [a, b, c, d];
        "#;
        let result = execute_math(source)?;
        let TypedList::Int(values) = runtime_list(result.first_return(), result.state.heap()) else {
            panic!("expected typed int list");
        };
        assert_eq!(values, &vec![100, 0, 4, 3]);
        Ok(())
    }

    #[test]
    fn test_math_clamp_duplicate_named_argument_error() {
        let named_args = [
            RuntimeVal::ShortStr(ShortStr::new("min").expect("short")),
            RuntimeVal::Int(0),
            RuntimeVal::ShortStr(ShortStr::new("min").expect("short")),
            RuntimeVal::Int(1),
        ];
        let err = call_math_named_stack("clamp", &[RuntimeVal::Int(5)], &named_args, 2)
            .expect_err("duplicate named arguments should error");
        assert!(err.to_string().contains("duplicate named argument"));
    }

    #[test]
    fn test_math_sqrt_negative_error() {
        let err = call_math("sqrt", &[RuntimeVal::Int(-1)]).expect_err("negative input should fail");
        assert!(err.to_string().contains("must be non-negative"));
    }

    #[test]
    fn test_math_log_non_positive_error() {
        let err = call_math("log", &[RuntimeVal::Int(0)]).expect_err("non-positive input should fail");
        assert!(err.to_string().contains("must be positive"));
    }

    #[test]
    fn test_math_atan2_mixed_numeric_types() -> Result<()> {
        let result = call_math("atan2", &[RuntimeVal::Int(1), RuntimeVal::Float(0.0)])?;
        let RuntimeVal::Float(angle) = result else {
            panic!("atan2 should return float");
        };
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-10);
        Ok(())
    }

    #[test]
    fn test_math_selected_functions_use_runtime_native_abi() -> Result<()> {
        for name in [
            "abs", "sqrt", "sin", "cos", "tan", "asin", "acos", "atan", "atan2", "log", "log10", "log2", "exp", "pow",
            "floor", "ceil", "round", "min", "max", "random",
        ] {
            let (arity, function) = math_native(name)?;
            assert!(
                matches!(function, NativeFunction::Plain(_)),
                "{name} should use plain RuntimeNative"
            );
            assert_ne!(
                arity,
                NativeEntry::VARIADIC,
                "{name} should have fixed positional arity"
            );
        }

        let (arity, function) = math_native("clamp")?;
        assert!(matches!(function, NativeFunction::Plain(_)));
        assert_eq!(arity, NativeEntry::VARIADIC);
        Ok(())
    }

    #[test]
    fn test_math_runtime_result_preserves_runtime_value() -> Result<()> {
        let result = call_math("min", &[RuntimeVal::Float(2.5), RuntimeVal::Int(3)])?;
        assert_eq!(result, RuntimeVal::Float(2.5));
        Ok(())
    }
}
