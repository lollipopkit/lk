#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{register_stdlib_modules, string::StringModule};
    use anyhow::Result;
    use lkr_core::{
        module::{Module, ModuleRegistry},
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::Val,
        vm::{Vm, VmContext},
    };

    #[test]
    fn test_string_len() -> Result<()> {
        let source = "import string; return string.len(\"hello\");";
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
        assert_eq!(result, Val::Int(5));

        Ok(())
    }

    #[test]
    fn test_string_lower() -> Result<()> {
        let source = "import string; return string.lower(\"HELLO\");";
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
        assert_eq!(result, Val::Str("hello".into()));

        Ok(())
    }

    #[test]
    fn test_string_method_sugar() -> Result<()> {
        let source = "return \"hello\".len();";
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        // Create registry and register stdlib modules (ensures methods are registered)
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;

        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut machine = Vm::new();

        let result = program.execute_with_vm(&mut machine, &mut env)?;
        assert_eq!(result, Val::Int(5));
        Ok(())
    }

    #[test]
    fn test_string_replace_named_arguments() -> Result<()> {
        let source = r#"
            import string;
            let named = string.replace("lollipop", pattern: "l", with: "x");
            let named_all = string.replace("lollipop", pattern: "l", with: "x", all: true);
            let legacy = string.replace("lollipop", "l", "x");
            return [named, named_all, legacy];
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
        let expected = Val::List(
            vec![
                Val::Str("xollipop".into()),
                Val::Str("xoxxipop".into()),
                Val::Str("xoxxipop".into()),
            ]
            .into(),
        );
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_string_replace_duplicate_named_argument_error() {
        let module = StringModule::new();
        let Val::RustFunctionNamed(replace_fn) =
            module.exports().get("replace").expect("replace export present").clone()
        else {
            panic!("replace should be a named Rust function");
        };

        let mut env = VmContext::new();
        let named_args = vec![
            ("pattern".to_string(), Val::Str("l".into())),
            ("pattern".to_string(), Val::Str("x".into())),
            ("with".to_string(), Val::Str("a".into())),
        ];

        let err = replace_fn(&[Val::Str("lol".into())], &named_args, &mut env)
            .expect_err("duplicate named arguments should error");
        assert!(err.to_string().contains("duplicate named argument"));
    }

    #[test]
    fn test_string_substring_out_of_bounds_error() {
        let source = "import string; return string.substring(\"abc\", 10, 1);";
        let tokens = Tokenizer::tokenize(source).expect("tokenize substring source");
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program().expect("parse substring source");

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry).expect("register stdlib");
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut machine = Vm::new();
        let err = program
            .execute_with_vm(&mut machine, &mut env)
            .expect_err("out-of-bounds substring should error");
        assert!(err.to_string().contains("start index out of bounds"));
    }

    #[test]
    fn test_string_join_rejects_non_string_items() {
        let source = "import string; return string.join([\"ok\", 123], \",\");";
        let tokens = Tokenizer::tokenize(source).expect("tokenize join source");
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program().expect("parse join source");

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry).expect("register stdlib");
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut machine = Vm::new();
        let err = program
            .execute_with_vm(&mut machine, &mut env)
            .expect_err("non-string list elements should error");
        assert!(err.to_string().contains("list must contain only strings"));
    }
}
