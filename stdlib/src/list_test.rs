#[cfg(test)]
mod tests {
    use crate::{list::ListModule, register_stdlib_modules};
    use anyhow::Result;
    use lkr_core::{
        module::{self, Module},
        stmt::{self, stmt_parser::StmtParser},
        token::Tokenizer,
        val::Val,
        vm::{self, VmContext},
    };
    use std::sync::Arc;

    fn run(source: &str) -> Result<Val> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = module::ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(stmt::ModuleResolver::with_registry(registry));
        let mut env = vm::VmContext::new().with_resolver(resolver);
        let mut machine = vm::Vm::new();
        program.execute_with_vm(&mut machine, &mut env)
    }

    #[test]
    fn test_list_len_method_sugar() -> Result<()> {
        let v = run("return [1,2,3].len();")?;
        assert_eq!(v, Val::Int(3));
        Ok(())
    }

    #[test]
    fn test_list_push_join() -> Result<()> {
        let v = run("return [\"a\", \"b\"].push(\"c\").join(\",\");")?;
        assert_eq!(v, Val::Str("a,b,c".into()));
        Ok(())
    }

    #[test]
    fn test_list_get_first_last() -> Result<()> {
        assert_eq!(run("return [10,20,30].get(1);")?, Val::Int(20));
        assert_eq!(run("return [10,20,30].get(5);")?, Val::Nil);
        assert_eq!(run("return [10,20,30].first();")?, Val::Int(10));
        assert_eq!(run("return [10,20,30].last();")?, Val::Int(30));
        Ok(())
    }

    #[test]
    fn test_list_map_filter_reduce() -> Result<()> {
        // map
        assert_eq!(
            run("return [1,2,3].map(|x| x + 1);")?,
            Val::List(Arc::from(vec![Val::Int(2), Val::Int(3), Val::Int(4)]))
        );

        // filter
        assert_eq!(
            run("return [1,2,3,4,5].filter(|x| x % 2 == 0);")?,
            Val::List(Arc::from(vec![Val::Int(2), Val::Int(4)]))
        );

        // reduce (sum)
        assert_eq!(run("return [1,2,3,4].reduce(0, |acc, x| acc + x);")?, Val::Int(10));
        Ok(())
    }

    #[test]
    fn test_list_get_out_of_bounds_yields_nil() -> Result<()> {
        let result =
            run("import list; let data = [1, 2]; return [list.get(data, 1), list.get(data, 5), list.get(data, -1)];")?;
        let Val::List(values) = result else {
            panic!("expected list result");
        };
        assert_eq!(values.len(), 3);
        assert_eq!(values[0], Val::Int(2));
        assert_eq!(values[1], Val::Nil);
        assert_eq!(values[2], Val::Nil);
        Ok(())
    }

    #[test]
    fn test_list_push_method_sugar_preserves_original() -> Result<()> {
        let result = run(
            "import list; let original = [1, 2]; let appended = original.push(3); return [original.len(), appended.len(), appended.get(2)];",
        )?;
        let Val::List(values) = result else {
            panic!("expected list result");
        };
        assert_eq!(values.len(), 3);
        assert_eq!(values[0], Val::Int(2));
        assert_eq!(values[1], Val::Int(3));
        assert_eq!(values[2], Val::Int(3));
        Ok(())
    }

    #[test]
    fn test_list_get_rejects_non_integer_index() {
        let module = ListModule::new();
        let Val::RustFunction(get_fn) = module.exports().get("get").expect("get function present").clone() else {
            panic!("expected get to be a RustFunction");
        };

        let list_val = Val::List(vec![Val::Int(1)].into());
        let mut env = VmContext::new();
        let err = get_fn(&[list_val, Val::Str("x".into())], &mut env).expect_err("non-integer index should error");
        assert!(err.to_string().contains("index must be an integer"));
    }

    #[test]
    fn test_list_first_last_empty_return_nil() -> Result<()> {
        let result = run("import list; return [list.first([]), list.last([])];")?;
        let Val::List(values) = result else {
            panic!("expected list result");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], Val::Nil);
        assert_eq!(values[1], Val::Nil);
        Ok(())
    }

    #[test]
    fn test_list_set_returns_pair() -> Result<()> {
        let result = run("import list; let pair = list.set([1, 2, 3], 1, 42); \
             let updated = pair.get(0); \
             let old = pair.get(1); \
             return [updated.get(1), old];")?;
        let Val::List(values) = result else {
            panic!("expected list result");
        };
        assert_eq!(values.len(), 2);
        assert_eq!(values[0], Val::Int(42));
        assert_eq!(values[1], Val::Int(2));
        Ok(())
    }
}
