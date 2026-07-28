#[cfg(test)]
mod tests {
    use lk_core::vm::ModuleResolver;
    use lk_core::vm::ProgramExec;
    use std::sync::Arc;

    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        val::RuntimeVal,
        vm::{ProgramResult, VmContext},
    };

    fn run(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        crate::register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    #[test]
    fn io_file_module_imports_from_parent_namespace() -> Result<()> {
        let mut path = std::env::temp_dir();
        path.push(format!("lk-io-file-test-{}.txt", std::process::id()));
        let path = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let source = format!(
            r#"
            use fs;
            use {{ file }} from io;
            let writer = file.open("{path}", "write");
            file.write(writer, "hello");
            file.close(writer);
            let reader = file.open("{path}", "read");
            let content = file.read_to_string(reader);
            file.close(reader);
            let exists = fs.exists("{path}");
            let size = fs.metadata("{path}").len;
            fs.remove_file("{path}");
            return exists && content == "hello" && size == 5;
            "#
        );
        let result = run(&source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    #[test]
    fn parent_namespaces_are_importable_as_modules() -> Result<()> {
        let mut path = std::env::temp_dir();
        path.push(format!("lk-io-parent-test-{}.txt", std::process::id()));
        let path = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let source = format!(
            r#"
            use fs;
            use io;
            use {{ socket }} from net;
            let writer = io.file.open("{path}", "write");
            io.file.write(writer, "hello");
            io.file.close(writer);
            let reader = io.file.open("{path}", "read");
            let content = io.file.read_to_string(reader);
            io.file.close(reader);
            let addr = socket.addr("127.0.0.1", 80);
            fs.remove_file("{path}");
            return content == "hello" && addr == "127.0.0.1:80" && typeof(io.std.stdout()) == "Stdout";
            "#
        );
        let result = run(&source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    #[test]
    fn list_windows_stay_views_until_materialized() -> Result<()> {
        // Was `slice_module_keeps_views_until_materialization`, against a
        // `slice` module that has been removed: taking a window over a list is
        // something the list does, not a module you import first.
        let source = r#"
            let xs = [1, 2, 3, 4];
            let view = xs.slice(1, 3);
            return view.len() == 2
                && view[0] == 2
                && view.to_list() == [2, 3]
                && view.slice(1, 2).to_list() == [3];
        "#;
        let result = run(source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    /// `iter.f(xs, ...)` and `xs.f(...)` are the same operation, and this is
    /// what says so.
    ///
    /// They used to be two implementations — the module's own snapshotting,
    /// truthiness and result-building beside `core_methods`' — and they agreed
    /// on everything checked here, which is exactly why nobody noticed that
    /// `take(-1)` did not: the method form cast `-1` to `usize` and returned
    /// the whole list, the module form raised. Comparing them element by
    /// element is the only thing that would have found it, so it lives here
    /// now rather than in whoever's memory.
    #[test]
    fn the_iter_module_is_a_spelling_of_the_list_methods() -> Result<()> {
        let source = r#"
            use iter;
            let xs = [1, 2, 3, 4, 5];
            let d = [3, 1, 3, 2, 1];
            let n = [[1, 2], [3], [4, [5, 6]]];
            return iter.map(xs, |x| x * 2) == xs.map(|x| x * 2)
                && iter.filter(xs, |x| x % 2 == 0) == xs.filter(|x| x % 2 == 0)
                && iter.reduce(xs, 0, |a, b| a + b) == xs.reduce(0, |a, b| a + b)
                && iter.enumerate(xs) == xs.enumerate()
                && iter.zip(xs, d) == xs.zip(d)
                && iter.take(xs, 2) == xs.take(2)
                && iter.take(xs, 99) == xs.take(99)
                && iter.skip(xs, 2) == xs.skip(2)
                && iter.skip(xs, 99) == xs.skip(99)
                && iter.chain(xs, d) == xs.chain(d)
                && iter.flatten(n) == n.flatten()
                && iter.unique(d) == d.unique()
                && iter.chunk(xs, 2) == xs.chunk(2)
                && iter.next(xs) == xs.first();
        "#;
        let result = run(source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    /// A count is not an index: there is nothing for a negative one to mean.
    ///
    /// Both spellings raise, with the same text — the method form used to
    /// answer `[1, 2, 3]` here, by way of `-1 as usize`.
    #[test]
    fn a_negative_take_or_skip_count_raises_in_both_spellings() {
        for source in [
            "let xs = [1, 2, 3]; return xs.take(0 - 1);",
            "use iter; let xs = [1, 2, 3]; return iter.take(xs, 0 - 1);",
            "let xs = [1, 2, 3]; return xs.skip(0 - 1);",
            "use iter; let xs = [1, 2, 3]; return iter.skip(xs, 0 - 1);",
        ] {
            let error = run(source).expect_err(&format!("`{source}` must raise"));
            let text = format!("{error:#}");
            assert!(
                text.contains("count must be non-negative, got -1"),
                "`{source}` raised the wrong thing: {text}"
            );
        }
    }
}
