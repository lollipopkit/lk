#[cfg(test)]
mod tests {
    use anyhow::Result;
    use lk_core::{
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal, TypedList},
        vm::{NativeArgs, NativeFunction, NativeRuntime, ProgramResult, RuntimeModuleState, VmContext},
    };
    use std::sync::Arc;

    fn run(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = lk_core::module::ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        crate::register_stdlib_globals(&mut registry);
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn stream_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&crate::stream::StreamModule::new(), name)
    }

    fn expect_list(value: &RuntimeVal, heap: &HeapStore) -> Vec<RuntimeVal> {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(*handle) else {
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

    #[test]
    fn test_stream_range_map_take_collect() -> Result<()> {
        let v = run(
            "use stream; let s = stream.take(stream.map(stream.range(0, 10), fn(x) => x * 2), 3); let c = stream.subscribe(s); return stream.collect(c);",
        )?;
        assert_eq!(
            expect_list(v.first_return(), v.state.heap()),
            vec![RuntimeVal::Int(0), RuntimeVal::Int(2), RuntimeVal::Int(4)]
        );
        Ok(())
    }

    #[test]
    fn test_stream_iterate_infinite_take() -> Result<()> {
        let v =
            run("use stream; let s = stream.take(stream.iterate(1, fn(x) => x + 1), 5); return stream.collect(s);")?;
        assert_eq!(
            expect_list(v.first_return(), v.state.heap()),
            vec![
                RuntimeVal::Int(1),
                RuntimeVal::Int(2),
                RuntimeVal::Int(3),
                RuntimeVal::Int(4),
                RuntimeVal::Int(5)
            ]
        );
        Ok(())
    }

    #[test]
    fn test_list_to_stream_collect() -> Result<()> {
        let v = run("use stream; return stream.collect(stream.from_list([1,2,3]));")?;
        assert_eq!(
            expect_list(v.first_return(), v.state.heap()),
            vec![RuntimeVal::Int(1), RuntimeVal::Int(2), RuntimeVal::Int(3)]
        );
        Ok(())
    }

    #[test]
    fn test_stream_from_channel_next() -> Result<()> {
        let v = run(
            "use stream; let ch = chan(8); send(ch, 42); let s = stream.from_channel(ch); let c = stream.subscribe(s); return stream.next(c);",
        )?;
        let values = expect_list(v.first_return(), v.state.heap());
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], RuntimeVal::Bool(true));
        assert_eq!(values[1], RuntimeVal::Int(42));
        Ok(())
    }

    #[test]
    fn test_stream_module_exports_use_runtime_native_abi() -> Result<()> {
        let module = crate::stream::StreamModule::new();
        for name in ["next", "collect", "next_block", "collect_block"] {
            let (_, function) = crate::runtime_native::runtime_native_export(&module, name)?;
            assert!(matches!(function, NativeFunction::FullState(_)));
        }
        for name in [
            "from_list",
            "range",
            "iterate",
            "repeat",
            "from_channel",
            "map",
            "filter",
            "take",
            "skip",
            "chain",
            "subscribe",
        ] {
            let (_, function) = crate::runtime_native::runtime_native_export(&module, name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
        }
        Ok(())
    }

    #[test]
    fn stream_from_list_preserves_typed_string_backing_without_materializing_items() -> Result<()> {
        let (_, function) = stream_native("from_list")?;
        let NativeFunction::Plain(function) = function else {
            panic!("from_list must use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let input = state.heap_mut().alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-stream-one"),
            Arc::<str>::from("long-stream-two"),
        ])));
        let args = [RuntimeVal::Obj(input)];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = function(NativeArgs::new(&args), &mut runtime)?;

        assert!(matches!(result, RuntimeVal::Obj(_)));
        assert_eq!(runtime.heap().len(), 2);
        Ok(())
    }

    #[test]
    fn stream_collect_preserves_typed_string_backing_without_materializing_items() -> Result<()> {
        let (_, from_list) = stream_native("from_list")?;
        let NativeFunction::Plain(from_list) = from_list else {
            panic!("from_list must use plain RuntimeNative");
        };
        let (_, collect) = stream_native("collect")?;
        let NativeFunction::FullState(collect) = collect else {
            panic!("collect must use full-state RuntimeNative");
        };
        let mut state = RuntimeModuleState::default();
        let input = state.heap_mut().alloc(HeapValue::List(TypedList::String(vec![
            Arc::<str>::from("long-stream-one"),
            Arc::<str>::from("long-stream-two"),
        ])));
        let stream = {
            let args = [RuntimeVal::Obj(input)];
            let mut runtime = NativeRuntime::new(&mut state, None, None);
            from_list(NativeArgs::new(&args), &mut runtime)?
        };
        let args = [stream];
        let mut runtime = NativeRuntime::new(&mut state, None, None);

        let result = collect(NativeArgs::new(&args), &mut runtime)?;

        let RuntimeVal::Obj(handle) = result else {
            panic!("expected list result");
        };
        let Some(HeapValue::List(TypedList::String(values))) = runtime.heap().get(handle) else {
            panic!("expected typed string list result");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(runtime.heap().len(), 4);
        Ok(())
    }
}
