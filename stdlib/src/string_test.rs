#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{register_stdlib_modules, runtime_native::runtime_string_value, string::StringModule};
    use anyhow::{Result, anyhow};
    use lk_core::{
        module::{Module, ModuleRegistry},
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{CallableValue, HeapStore, HeapValue, RuntimeVal, Val},
        vm::{NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, RuntimeModuleState32, VmContext},
    };

    fn execute_string32(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute32_with_ctx(&mut env)
    }

    fn string_native(name: &str) -> Result<(u16, NativeFunction32)> {
        let exports = StringModule::new().exports();
        let value = exports.get(name).ok_or_else(|| anyhow!("{name} export present"))?;
        let Val::Obj(object) = value else {
            return Err(anyhow!("{name} must be a heap callable"));
        };
        let HeapValue::Callable(CallableValue::RuntimeNative32 { arity, function }) = object.as_ref() else {
            return Err(anyhow!("{name} must be RuntimeNative32"));
        };
        Ok((*arity, function.clone()))
    }

    #[test]
    fn test_string_len() -> Result<()> {
        let result = execute_string32("import string; return string.len(\"hello\");")?;
        assert_eq!(result, Val::Int(5));

        Ok(())
    }

    #[test]
    fn test_string_lower() -> Result<()> {
        let result = execute_string32("import string; return string.lower(\"HELLO\");")?;
        assert_eq!(result, Val::from_str("hello"));

        Ok(())
    }

    #[test]
    fn test_string_method_sugar() -> Result<()> {
        let result = execute_string32("return \"hello\".len();")?;
        assert_eq!(result, Val::Int(5));
        Ok(())
    }

    #[test]
    fn test_string_functions_use_runtime_native32_abi() -> Result<()> {
        for name in [
            "len",
            "lower",
            "upper",
            "trim",
            "starts_with",
            "ends_with",
            "contains",
            "substring",
            "split",
            "join",
            "reverse",
            "repeat",
            "char",
            "byte",
            "chars",
            "is_empty",
        ] {
            let (arity, function) = string_native(name)?;
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
        for name in ["replace", "find", "format"] {
            let (arity, function) = string_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
            assert_eq!(arity, NativeEntry32::VARIADIC);
        }
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
        let result = execute_string32(source)?;
        let expected = Val::list(
            vec![
                Val::from_str("xollipop"),
                Val::from_str("xoxxipop"),
                Val::from_str("xoxxipop"),
            ]
            .into(),
        );
        assert_eq!(result, expected);
        Ok(())
    }

    #[test]
    fn test_string_replace_duplicate_named_argument_error() {
        let mut heap = HeapStore::new();
        let source = runtime_string_value("lol", &mut heap);
        let named_args = vec![
            ("pattern".to_string(), runtime_string_value("l", &mut heap)),
            ("pattern".to_string(), runtime_string_value("x", &mut heap)),
            ("with".to_string(), runtime_string_value("a", &mut heap)),
        ];
        let (_, function) = string_native("replace").expect("replace native");
        let NativeFunction32::Plain(function) = function else {
            panic!("replace should use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32 {
            heap,
            globals: Vec::new(),
        };
        let mut runtime = NativeRuntime32 {
            state: &mut state,
            ctx: None,
            module: None,
        };
        let err = function(NativeArgs32::new_with_named(&[source], &named_args), &mut runtime)
            .expect_err("duplicate named arguments should error");
        assert!(err.to_string().contains("duplicate named argument"));
    }

    #[test]
    fn test_string_substring_out_of_bounds_error() {
        let source = "import string; return string.substring(\"abc\", 10, 1);";
        let err = execute_string32(source).expect_err("out-of-bounds substring should error");
        assert!(err.to_string().contains("start index out of bounds"));
    }

    #[test]
    fn test_string_join_rejects_non_string_items() {
        let source = "import string; return string.join([\"ok\", 123], \",\");";
        let err = execute_string32(source).expect_err("non-string list elements should error");
        assert!(err.to_string().contains("list must contain only strings"));
    }

    #[test]
    fn test_string_runtime_direct_call_with_heap_string() -> Result<()> {
        let mut heap = HeapStore::new();
        let input = runtime_string_value("hello", &mut heap);
        let suffix = runtime_string_value("lo", &mut heap);
        let (_, function) = string_native("ends_with")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("ends_with should use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32 {
            heap,
            globals: Vec::new(),
        };
        let mut runtime = NativeRuntime32 {
            state: &mut state,
            ctx: None,
            module: None,
        };
        let result = function(NativeArgs32::new(&[input, suffix]), &mut runtime)?;
        assert_eq!(result, RuntimeVal::Bool(true));
        Ok(())
    }
}
