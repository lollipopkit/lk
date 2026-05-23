#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{list::ListModule, register_stdlib_modules, runtime_native::runtime_string_value};
    use anyhow::Result;
    use lk_core::{
        module::{self},
        stmt::{self, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal, TypedList},
        vm::{self, NativeArgs32, NativeFunction32, NativeRuntime32, Program32Result, RuntimeModuleState32},
    };

    fn run32(source: &str) -> Result<Program32Result> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = module::ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);
        program.execute32_with_ctx(&mut env)
    }

    fn list_native(name: &str) -> Result<(u16, NativeFunction32)> {
        crate::runtime_native::runtime_native_export(&ListModule::new(), name)
    }

    fn expect_runtime_list(value: RuntimeVal, heap: &HeapStore) -> Vec<RuntimeVal> {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(handle) else {
            panic!("expected runtime list heap value");
        };
        match list {
            TypedList::Mixed(values) => values.clone(),
            TypedList::Int(values) => values.iter().copied().map(RuntimeVal::Int).collect(),
            TypedList::Float(values) => values.iter().copied().map(RuntimeVal::Float).collect(),
            TypedList::Bool(values) => values.iter().copied().map(RuntimeVal::Bool).collect(),
            TypedList::String(values) => values
                .iter()
                .map(|value| RuntimeVal::ShortStr(lk_core::val::ShortStr::new(value).expect("short test string")))
                .collect(),
        }
    }

    fn expect_runtime_list_backing(value: RuntimeVal, heap: &HeapStore) -> TypedList {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(handle) else {
            panic!("expected runtime list heap value");
        };
        list.clone()
    }

    #[test]
    fn test_list_len_push_join() -> Result<()> {
        assert_eq!(
            run32("import list; return list.len([1,2,3]);")?.first_return(),
            &RuntimeVal::Int(3)
        );
        assert_eq!(
            run32("import list; return list.join(list.push([\"a\", \"b\"], \"c\"), \",\");")?.first_return(),
            &RuntimeVal::ShortStr(lk_core::val::ShortStr::new("a,b,c").expect("short string"))
        );
        Ok(())
    }

    #[test]
    fn test_list_get_first_last() -> Result<()> {
        assert_eq!(
            run32("import list; return list.get([10,20,30], 1);")?.first_return(),
            &RuntimeVal::Int(20)
        );
        assert_eq!(
            run32("import list; return list.get([10,20,30], 5);")?.first_return(),
            &RuntimeVal::Nil
        );
        assert_eq!(
            run32("import list; return list.get([10,20,30], -1);")?.first_return(),
            &RuntimeVal::Nil
        );
        assert_eq!(
            run32("import list; return list.first([10,20,30]);")?.first_return(),
            &RuntimeVal::Int(10)
        );
        assert_eq!(
            run32("import list; return list.last([10,20,30]);")?.first_return(),
            &RuntimeVal::Int(30)
        );
        let result = run32("import list; return [list.first([]), list.last([])];")?;
        assert_eq!(
            expect_runtime_list(result.first_return().clone(), &result.state.heap),
            vec![RuntimeVal::Nil, RuntimeVal::Nil]
        );
        Ok(())
    }

    #[test]
    fn test_list_concat_and_set_returns_pair() -> Result<()> {
        let concat = run32("import list; return list.concat([1,2], [3,4]);")?;
        assert_eq!(
            expect_runtime_list(concat.first_return().clone(), &concat.state.heap),
            vec![
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(3),
                RuntimeVal::Int(4)
            ]
        );
        let result = run32(
            "import list; let pair = list.set([1, 2, 3], 1, 42); \
             let updated = pair[0]; \
             let old = pair[1]; \
             return [updated[1], old];",
        )?;
        assert_eq!(
            expect_runtime_list(result.first_return().clone(), &result.state.heap),
            vec![RuntimeVal::Int(42), RuntimeVal::Int(2)]
        );
        Ok(())
    }

    #[test]
    fn test_list_get_rejects_non_integer_index() {
        let err = run32("import list; return list.get([1], \"x\");").expect_err("non-integer index should error");
        assert!(err.to_string().contains("index must be an integer"));
    }

    #[test]
    fn test_list_join_rejects_non_string_items() {
        let err =
            run32("import list; return list.join([\"ok\", 1], \",\");").expect_err("non-string items should error");
        assert!(err.to_string().contains("list must contain only strings"));
    }

    #[test]
    fn test_list_public_functions_use_runtime_native32_abi() -> Result<()> {
        for name in ["len", "push", "concat", "join", "get", "first", "last", "set"] {
            let (arity, function) = list_native(name)?;
            assert!(matches!(function, NativeFunction32::Plain(_)));
            assert_ne!(arity, lk_core::vm::NativeEntry32::VARIADIC);
        }
        Ok(())
    }

    #[test]
    fn test_list_direct_runtime_call_preserves_typed_backing() -> Result<()> {
        let (_, function) = list_native("push")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("push should use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let list = RuntimeVal::Obj(state.heap.alloc(HeapValue::List(TypedList::Int(vec![1, 2]))));
        let args = [list, RuntimeVal::Int(3)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        assert!(matches!(
            expect_runtime_list_backing(result.clone(), runtime.heap()),
            TypedList::Int(values) if values == vec![1, 2, 3]
        ));
        assert_eq!(
            expect_runtime_list(result, runtime.heap()),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2), RuntimeVal::Int(3)]
        );
        Ok(())
    }

    #[test]
    fn test_list_direct_runtime_set_preserves_typed_backing() -> Result<()> {
        let (_, function) = list_native("set")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("set should use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let list = RuntimeVal::Obj(state.heap.alloc(HeapValue::List(TypedList::Int(vec![1, 2]))));
        let args = [list, RuntimeVal::Int(1), RuntimeVal::Int(7)];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        let pair = expect_runtime_list(result, runtime.heap());
        assert_eq!(pair[1], RuntimeVal::Int(2));
        assert!(matches!(
            expect_runtime_list_backing(pair[0].clone(), runtime.heap()),
            TypedList::Int(values) if values == vec![1, 7]
        ));
        Ok(())
    }

    #[test]
    fn test_list_direct_runtime_concat_preserves_typed_backing() -> Result<()> {
        let (_, function) = list_native("concat")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("concat should use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let left = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from("a")]))),
        );
        let right = RuntimeVal::Obj(
            state
                .heap
                .alloc(HeapValue::List(TypedList::String(vec![Arc::<str>::from("b")]))),
        );
        let args = [left, right];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        assert!(matches!(
            expect_runtime_list_backing(result, runtime.heap()),
            TypedList::String(values) if values.iter().map(|value| value.as_ref()).collect::<Vec<_>>() == vec!["a", "b"]
        ));
        Ok(())
    }

    #[test]
    fn test_list_direct_runtime_join_with_heap_strings() -> Result<()> {
        let (_, function) = list_native("join")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("join should use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32::default();
        let list = RuntimeVal::Obj(state.heap.alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("a"),
            Arc::<str>::from("b"),
        ]))));
        let delimiter = runtime_string_value(",", &mut state.heap);
        let args = [list, delimiter];
        let mut runtime = NativeRuntime32::new(&mut state, None, None);
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        assert!(matches!(result, RuntimeVal::ShortStr(value) if value.as_str() == "a,b"));
        Ok(())
    }
}
