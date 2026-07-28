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
}
