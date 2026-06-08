#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
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
    fn bytes_module_covers_binary_primitives() -> Result<()> {
        let source = r#"
            use bytes;
            let a = bytes.from_list([65, 66, 67]);
            let b = bytes.from_string("de");
            let c = bytes.concat(a, b);
            return typeof(a) == "Bytes"
                && bytes.len(a) == 3
                && !bytes.is_empty(a)
                && bytes.get(a, 0) == 65
                && bytes.get(a, 99) == nil
                && bytes.to_list(bytes.slice(c, 1, 4)) == [66, 67, 100]
                && bytes.to_string_utf8(c) == "ABCde"
                && bytes.to_string_lossy(bytes.from_list([255])) != ""
                && bytes.eq(a, bytes.from_list([65, 66, 67]));
        "#;

        let result = run(source)?;

        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    #[test]
    fn bytes_from_list_rejects_non_byte_values() {
        let err = run("use bytes; return bytes.from_list([256]);").expect_err("256 is outside u8 range");
        assert!(err.to_string().contains("0..255"));

        let err = run("use bytes; return bytes.from_list([-1]);").expect_err("-1 is outside u8 range");
        assert!(err.to_string().contains("0..255"));

        let err = run("use bytes; return bytes.from_list([\"x\"]);").expect_err("non-int item should fail");
        assert!(err.to_string().contains("expects Int items"));
    }

    #[test]
    fn file_read_roundtrips_bytes_and_text_api_remains_explicit() -> Result<()> {
        let mut path = std::env::temp_dir();
        path.push(format!("lk-bytes-file-test-{}.bin", std::process::id()));
        let path = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let source = format!(
            r#"
            use bytes;
            use {{ file }} from io;
            let data = bytes.from_list([0, 65, 255]);
            file.write("{path}", data);
            let raw = file.read("{path}");
            let text_path = "{path}.txt";
            file.write(text_path, bytes.from_string("hello"));
            let text = file.read_to_string(text_path);
            file.remove("{path}");
            file.remove(text_path);
            return bytes.eq(raw, data) && text == "hello";
            "#
        );

        let result = run(&source)?;

        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }
}
