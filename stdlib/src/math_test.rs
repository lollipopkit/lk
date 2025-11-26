#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{math::MathModule, register_stdlib_modules};
    use anyhow::Result;
    use lkr_core::{
        module::{Module, ModuleRegistry},
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::Val,
        vm::{Vm, VmContext},
    };

    #[test]
    fn test_math_abs_positive() -> Result<()> {
        let source = "import math; return math.abs(42);";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Create registry and register stdlib modules
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;

        // Create environment with stdlib modules
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut machine = Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        assert_eq!(result, Val::Int(42));

        Ok(())
    }

    #[test]
    fn test_math_abs_negative() -> Result<()> {
        let source = "import math; return math.abs(-42);";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Create registry and register stdlib modules
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;

        // Create environment with stdlib modules
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut machine = Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        assert_eq!(result, Val::Int(42));

        Ok(())
    }

    #[test]
    fn test_math_sqrt() -> Result<()> {
        let source = "import math; return math.sqrt(16);";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Create registry and register stdlib modules
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;

        // Create environment with stdlib modules
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut machine = Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        assert_eq!(result, Val::Float(4.0));

        Ok(())
    }

    #[test]
    fn test_math_constants() -> Result<()> {
        let source = "import math; return math.pi;";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Create registry and register stdlib modules
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;

        // Create environment with stdlib modules
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut machine = Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        if let Val::Float(value) = result {
            assert!((value - std::f64::consts::PI).abs() < 1e-10);
        } else {
            panic!("Expected float result");
        }

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
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut machine = Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        let expected = Val::List(vec![Val::Int(100), Val::Int(0), Val::Int(4), Val::Int(3)].into());
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_math_clamp_duplicate_named_argument_error() {
        let module = MathModule::new();
        let Val::RustFunctionNamed(clamp_fn) = module.exports().get("clamp").expect("clamp export present").clone()
        else {
            panic!("expected clamp to be a named Rust function");
        };
        let mut env = VmContext::new();
        let named_args = vec![("min".to_string(), Val::Int(0)), ("min".to_string(), Val::Int(1))];
        let err = clamp_fn(&[Val::Int(5)], &named_args, &mut env).expect_err("duplicate named arguments should error");
        assert!(err.to_string().contains("duplicate named argument"));
    }

    #[test]
    fn test_math_sqrt_negative_error() {
        let module = MathModule::new();
        let Val::RustFunction(sqrt_fn) = module.exports().get("sqrt").expect("sqrt export present").clone() else {
            panic!("expected sqrt to be a Rust function");
        };
        let mut env = VmContext::new();
        let err = sqrt_fn(&[Val::Int(-1)], &mut env).expect_err("negative input should fail");
        assert!(err.to_string().contains("must be non-negative"));
    }

    #[test]
    fn test_math_log_non_positive_error() {
        let module = MathModule::new();
        let Val::RustFunction(log_fn) = module.exports().get("log").expect("log export present").clone() else {
            panic!("expected log to be a Rust function");
        };
        let mut env = VmContext::new();
        let err = log_fn(&[Val::Int(0)], &mut env).expect_err("non-positive input should fail");
        assert!(err.to_string().contains("must be positive"));
    }

    #[test]
    fn test_math_atan2_mixed_numeric_types() -> Result<()> {
        let module = MathModule::new();
        let Val::RustFunction(atan2_fn) = module.exports().get("atan2").expect("atan2 export present").clone() else {
            panic!("expected atan2 to be a Rust function");
        };
        let mut env = VmContext::new();
        let result = atan2_fn(&[Val::Int(1), Val::Float(0.0)], &mut env)?;
        let Val::Float(angle) = result else {
            panic!("atan2 should return float");
        };
        assert!((angle - std::f64::consts::FRAC_PI_2).abs() < 1e-10);
        Ok(())
    }
}
