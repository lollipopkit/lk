#[cfg(test)]
mod tests {
    use anyhow::Result;
    use lk_core::{
        module::Module,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{CallableValue, HeapValue, Val},
        vm::VmContext,
    };
    use std::sync::Arc;

    fn run(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = lk_core::module::ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        crate::register_stdlib_globals(&mut registry);
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute32_with_ctx(&mut env)
    }

    #[test]
    fn test_stream_range_map_take_collect() -> Result<()> {
        let v = run(
            "import stream; let s = stream.take(stream.map(stream.range(0, 10), fn(x) => x * 2), 3); let c = stream.subscribe(s); return stream.collect(c);",
        )?;
        assert_eq!(v, Val::list(Arc::from(vec![Val::Int(0), Val::Int(2), Val::Int(4)])));
        Ok(())
    }

    #[test]
    fn test_stream_iterate_infinite_take() -> Result<()> {
        let v =
            run("import stream; let s = stream.take(stream.iterate(1, fn(x) => x + 1), 5); return stream.collect(s);")?;
        assert_eq!(
            v,
            Val::list(Arc::from(vec![
                Val::Int(1),
                Val::Int(2),
                Val::Int(3),
                Val::Int(4),
                Val::Int(5)
            ]))
        );
        Ok(())
    }

    #[test]
    fn test_list_to_stream_collect() -> Result<()> {
        let v = run("import stream; return stream.collect(stream.from_list([1,2,3]));")?;
        assert_eq!(v, Val::list(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)])));
        Ok(())
    }

    #[test]
    fn test_stream_from_channel_next() -> Result<()> {
        let v = run(
            "import stream; let ch = chan(8); send(ch, 42); let s = stream.from_channel(ch); let c = stream.subscribe(s); return stream.next(c);",
        )?;
        match v {
            value if value.as_list().is_some() => {
                let l = value.as_list().expect("checked list");
                assert_eq!(l.len(), 2);
                assert_eq!(l[0], Val::Bool(true));
                assert_eq!(l[1], Val::Int(42));
            }
            _ => panic!("expected tuple [ok, value]"),
        }
        Ok(())
    }

    #[test]
    fn test_stream_module_exports_use_runtime_native32_abi() {
        let module = crate::stream::StreamModule::new();
        let exports = module.exports();
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
            "next",
            "collect",
            "next_block",
            "collect_block",
        ] {
            let value = exports.get(name).expect("stream function export present");
            let Val::Obj(object) = value else {
                panic!("{name} should be heap callable");
            };
            let HeapValue::Callable(CallableValue::RuntimeNative32 { .. }) = object.as_ref() else {
                panic!("{name} should use RuntimeNative32");
            };
        }
    }
}
