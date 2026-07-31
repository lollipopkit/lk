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
                && a == bytes.from_list([65, 66, 67]);
        "#;

        let result = run(source)?;

        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    #[test]
    fn bytes_from_list_rejects_non_byte_values() {
        let err = run("use bytes; return bytes.from_list([256]);").expect_err("256 is outside u8 range");
        assert!(err.to_string().contains("0..=255"));

        let err = run("use bytes; return bytes.from_list([-1]);").expect_err("-1 is outside u8 range");
        assert!(err.to_string().contains("0..=255"));

        let err = run("use bytes; return bytes.from_list([\"x\"]);").expect_err("non-int item should fail");
        assert!(err.to_string().contains("expects Int items"));

        // The method spelling of the same constructor, and the same refusal.
        let err = run("return [256].to_bytes();").expect_err("256 is outside u8 range");
        assert!(err.to_string().contains("0..=255"));
    }

    /// A reversed window is empty, not a refusal — the rule every other
    /// sequence reads by, and the one the *method* spelling always followed.
    ///
    /// `bytes.slice(b, 2, 1)` used to raise while `b.slice(2, 1)` answered
    /// `Bytes([])`: the same call, two bodies, two answers. The module forwards
    /// to the method now, so there is one answer and it is the clamping one
    /// (`"abcde".slice(-1, -3)` and `xs.slice(-1, -3)` are empty too).
    #[test]
    fn a_reversed_window_is_empty_on_both_spellings() -> Result<()> {
        let source = r#"
            use bytes;
            let b = bytes.from_string("abc");
            return bytes.slice(b, 2, 1) == b.slice(2, 1)
                && bytes.len(bytes.slice(b, 2, 1)) == 0;
        "#;
        assert_eq!(run(source)?.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    /// Every `bytes` member answers exactly what its method spelling answers —
    /// by construction, because the module forwards.
    ///
    /// Five members had two bodies (`len`, `is_empty`, `get`, `slice`,
    /// `to_list`), and `slice` had already drifted. Three more existed only as
    /// module functions and ten only as methods, so most of this surface could
    /// not even be *compared* until both spellings existed.
    #[test]
    fn every_module_spelling_answers_what_the_method_answers() -> Result<()> {
        let source = r#"
            use bytes;
            let b = bytes.from_string("abcde");
            return bytes.len(b) == b.len()
                && bytes.is_empty(b) == b.is_empty()
                && bytes.get(b, -1) == b.get(-1)
                && bytes.get(b, 99) == b.get(99)
                && bytes.first(b) == b.first()
                && bytes.last(b) == b.last()
                && bytes.contains(b, 98) == b.contains(98)
                && bytes.index_of(b, 98) == b.index_of(98)
                && bytes.sum(b) == b.sum()
                && bytes.min(b) == b.min()
                && bytes.max(b) == b.max()
                && bytes.take(b, 2) == b.take(2)
                && bytes.skip(b, 2) == b.skip(2)
                && bytes.slice(b, 1, 3) == b.slice(1, 3)
                && bytes.to_list(b) == b.to_list()
                && bytes.to_string_utf8(b) == b.to_string_utf8()
                && bytes.to_string_lossy(b) == b.to_string_lossy()
                && bytes.concat(b, b) == b.concat(b)
                && bytes.from_string("xy") == "xy".bytes()
                && bytes.from_list([1, 2]) == [1, 2].to_bytes();
        "#;
        assert_eq!(run(source)?.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    #[test]
    fn file_read_roundtrips_bytes_and_text_api_remains_explicit() -> Result<()> {
        let mut path = std::env::temp_dir();
        path.push(format!("lk-bytes-file-test-{}.bin", std::process::id()));
        let path = path.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let source = format!(
            r#"
            use bytes;
            use fs;
            use {{ file }} from io;
            let data = bytes.from_list([0, 65, 255]);
            fs.write("{path}", data);
            let raw = fs.read("{path}");
            let text_path = "{path}.txt";
            fs.write(text_path, bytes.from_string("hello"));
            let reader = file.open(text_path, "read");
            let text = file.read_to_string(reader);
            file.close(reader);
            fs.remove_file("{path}");
            fs.remove_file(text_path);
            return raw == data && text == "hello";
            "#
        );

        let result = run(&source)?;

        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }
}
