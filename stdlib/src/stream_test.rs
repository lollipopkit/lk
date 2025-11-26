#[cfg(test)]
mod tests {
    use anyhow::Result;
    use lkr_core::{
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::Val,
        vm::{Vm, VmContext},
    };
    use std::sync::Arc;

    fn run(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = lkr_core::module::ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        crate::register_stdlib_globals(&mut registry);
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        let mut vm = Vm::new();
        program.execute_with_vm(&mut vm, &mut env)
    }

    #[test]
    fn test_stream_range_map_take_collect() -> Result<()> {
        let v = run(
            "import stream; let s = stream.range(0, 10).map(|x| x * 2).take(3); let c = s.subscribe(); return c.collect();",
        )?;
        assert_eq!(v, Val::List(Arc::from(vec![Val::Int(0), Val::Int(2), Val::Int(4)])));
        Ok(())
    }

    #[test]
    fn test_stream_iterate_infinite_take() -> Result<()> {
        let v = run("import stream; let s = stream.iterate(1, |x| x + 1).take(5); return s.collect();")?;
        assert_eq!(
            v,
            Val::List(Arc::from(vec![
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
        let v = run("import stream; return [1,2,3].to_stream().collect();")?;
        assert_eq!(v, Val::List(Arc::from(vec![Val::Int(1), Val::Int(2), Val::Int(3)])));
        Ok(())
    }

    #[test]
    fn test_stream_from_channel_next() -> Result<()> {
        let v = run(
            "import stream; let ch = chan(8); send(ch, 42); let s = stream.from_channel(ch); let c = s.subscribe(); return c.next();",
        )?;
        match v {
            Val::List(l) => {
                assert_eq!(l.len(), 2);
                assert_eq!(l[0], Val::Bool(true));
                assert_eq!(l[1], Val::Int(42));
            }
            _ => panic!("expected tuple [ok, value]"),
        }
        Ok(())
    }
}
