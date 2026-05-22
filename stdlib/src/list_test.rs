#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{list::ListModule, register_stdlib_modules, runtime_native::runtime_string_value};
    use anyhow::{Result, anyhow};
    use lk_core::{
        module::{self, Module},
        stmt::{self, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{CallableValue, HeapStore, HeapValue, RuntimeVal, TypedList, Val, runtime_val_to_val},
        vm::{self, NativeArgs32, NativeFunction32, NativeRuntime32, RuntimeModuleState32},
    };

    fn run32(source: &str) -> Result<Val> {
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
        let exports = ListModule::new().exports();
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
    fn test_list_len_push_join() -> Result<()> {
        assert_eq!(run32("import list; return list.len([1,2,3]);")?, Val::Int(3));
        assert_eq!(
            run32("import list; return list.join(list.push([\"a\", \"b\"], \"c\"), \",\");")?,
            Val::from_str("a,b,c")
        );
        Ok(())
    }

    #[test]
    fn test_list_get_first_last() -> Result<()> {
        assert_eq!(run32("import list; return list.get([10,20,30], 1);")?, Val::Int(20));
        assert_eq!(run32("import list; return list.get([10,20,30], 5);")?, Val::Nil);
        assert_eq!(run32("import list; return list.get([10,20,30], -1);")?, Val::Nil);
        assert_eq!(run32("import list; return list.first([10,20,30]);")?, Val::Int(10));
        assert_eq!(run32("import list; return list.last([10,20,30]);")?, Val::Int(30));
        assert_eq!(run32("import list; return [list.first([]), list.last([])];")?, {
            Val::list(vec![Val::Nil, Val::Nil].into())
        });
        Ok(())
    }

    #[test]
    fn test_list_concat_and_set_returns_pair() -> Result<()> {
        assert_eq!(
            run32("import list; return list.concat([1,2], [3,4]);")?,
            Val::list(vec![Val::Int(1), Val::Int(2), Val::Int(3), Val::Int(4)].into())
        );
        let result = run32(
            "import list; let pair = list.set([1, 2, 3], 1, 42); \
             let updated = pair[0]; \
             let old = pair[1]; \
             return [updated[1], old];",
        )?;
        assert_eq!(result, Val::list(vec![Val::Int(42), Val::Int(2)].into()));
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
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let list = RuntimeVal::Obj(state.heap.alloc(HeapValue::List(TypedList::Int(vec![1, 2]))));
        let args = [list, RuntimeVal::Int(3)];
        let mut runtime = NativeRuntime32 {
            state: &mut state,
            ctx: None,
            module: None,
        };
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        let value = runtime_val_to_val(&result, &runtime.state.heap)?;
        assert_eq!(value, Val::list(vec![Val::Int(1), Val::Int(2), Val::Int(3)].into()));
        Ok(())
    }

    #[test]
    fn test_list_direct_runtime_join_with_heap_strings() -> Result<()> {
        let (_, function) = list_native("join")?;
        let NativeFunction32::Plain(function) = function else {
            panic!("join should use plain RuntimeNative32");
        };
        let mut state = RuntimeModuleState32 {
            heap: HeapStore::new(),
            globals: Vec::new(),
        };
        let list = RuntimeVal::Obj(state.heap.alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("a"),
            Arc::<str>::from("b"),
        ]))));
        let delimiter = runtime_string_value(",", &mut state.heap);
        let args = [list, delimiter];
        let mut runtime = NativeRuntime32 {
            state: &mut state,
            ctx: None,
            module: None,
        };
        let result = function(NativeArgs32::new(&args), &mut runtime)?;
        let value = runtime_val_to_val(&result, &runtime.state.heap)?;
        assert_eq!(value.as_str(), Some("a,b"));
        Ok(())
    }
}
