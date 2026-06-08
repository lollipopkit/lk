#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{register_stdlib_modules, runtime_native::runtime_string_value, string::StringModule};
    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal, ShortStr, TypedList},
        vm::{NativeArgs, NativeEntry, NativeFunction, NativeRuntime, ProgramResult, RuntimeModuleState, VmContext},
    };

    fn execute_string(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn string_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&StringModule::new(), name)
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
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(*handle) else {
            panic!("expected runtime list heap value");
        };
        list
    }

    #[test]
    fn test_string_len() -> Result<()> {
        let result = execute_string("use string; return string.len(\"hello\");")?;
        assert_eq!(result.first_return(), &RuntimeVal::Int(5));

        Ok(())
    }

    #[test]
    fn test_string_lower() -> Result<()> {
        let result = execute_string("use string; return string.lower(\"HELLO\");")?;
        assert_eq!(runtime_str(result.first_return(), result.state.heap()), Some("hello"));

        Ok(())
    }

    #[test]
    fn test_string_method_sugar() -> Result<()> {
        let result = execute_string("return \"hello\".len();")?;
        assert_eq!(result.first_return(), &RuntimeVal::Int(5));
        Ok(())
    }

    #[test]
    fn test_string_functions_use_runtime_native_abi() -> Result<()> {
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
                matches!(function, NativeFunction::Plain(_)),
                "{name} should use plain RuntimeNative"
            );
            assert_ne!(
                arity,
                NativeEntry::VARIADIC,
                "{name} should have fixed positional arity"
            );
        }
        for name in ["replace", "find", "format"] {
            let (arity, function) = string_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
            assert_eq!(arity, NativeEntry::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn test_string_replace_named_arguments() -> Result<()> {
        let source = r#"
            use string;
            let named = string.replace("lollipop", pattern: "l", with: "x");
            let named_all = string.replace("lollipop", pattern: "l", with: "x", all: true);
            let positional = string.replace("lollipop", "l", "x");
            return [named, named_all, positional];
        "#;
        let result = execute_string(source)?;
        let TypedList::String(values) = runtime_list(result.first_return(), result.state.heap()) else {
            panic!("expected typed string list");
        };
        assert_eq!(
            values.as_slice(),
            &[
                Arc::<str>::from("xollipop"),
                Arc::<str>::from("xoxxipop"),
                Arc::<str>::from("xoxxipop")
            ]
        );
        Ok(())
    }

    #[test]
    fn test_string_replace_duplicate_named_argument_error() {
        let mut heap = HeapStore::new();
        let source = runtime_string_value("lol", &mut heap);
        let named_args = [
            RuntimeVal::ShortStr(ShortStr::new("pattern").expect("short")),
            runtime_string_value("l", &mut heap),
            RuntimeVal::ShortStr(ShortStr::new("pattern").expect("short")),
            runtime_string_value("x", &mut heap),
            RuntimeVal::ShortStr(ShortStr::new("with").expect("short")),
            runtime_string_value("a", &mut heap),
        ];
        let (_, function) = string_native("replace").expect("replace native");
        let NativeFunction::Plain(function) = function else {
            panic!("replace should use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::new(heap, Vec::new());
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        let err = function(
            NativeArgs::new_with_named_stack(&[source], &named_args, 0, 3),
            &mut runtime,
        )
        .expect_err("duplicate named arguments should error");
        assert!(err.to_string().contains("duplicate named argument"));
    }

    #[test]
    fn test_string_substring_out_of_bounds_error() {
        let source = "use string; return string.substring(\"abc\", 10, 1);";
        let err = execute_string(source).expect_err("out-of-bounds substring should error");
        assert!(err.to_string().contains("start index out of bounds"));
    }

    #[test]
    fn test_string_join_rejects_non_string_items() {
        let source = "use string; return string.join([\"ok\", 123], \",\");";
        let err = execute_string(source).expect_err("non-string list elements should error");
        assert!(err.to_string().contains("list must contain only strings"));
    }

    #[test]
    fn test_string_runtime_direct_call_with_heap_string() -> Result<()> {
        let mut heap = HeapStore::new();
        let input = runtime_string_value("hello", &mut heap);
        let suffix = runtime_string_value("lo", &mut heap);
        let (_, function) = string_native("ends_with")?;
        let NativeFunction::Plain(function) = function else {
            panic!("ends_with should use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::new(heap, Vec::new());
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        let result = function(NativeArgs::new(&[input, suffix]), &mut runtime)?;
        assert_eq!(result, RuntimeVal::Bool(true));
        Ok(())
    }
}
