#[cfg(test)]
mod tests {
    use std::{fs::File, io::Write, sync::Arc};

    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::{ModuleResolver, stmt_parser::StmtParser},
        token::Tokenizer,
        val::{HeapValue, RuntimeVal, TypedList},
        vm::{ProgramResult, VmContext},
    };

    use crate::{StdlibExportKind, StdlibReturnKind, register_stdlib_modules, stdlib_catalog};

    fn run(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn runtime_list<'a>(value: &'a RuntimeVal, heap: &'a lk_core::val::HeapStore) -> &'a TypedList {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected list object");
        };
        let Some(HeapValue::List(values)) = heap.get(*handle) else {
            panic!("expected list heap value");
        };
        values
    }

    #[test]
    fn test_catalog_lowered_callables_have_return_kind() {
        let catalog = stdlib_catalog();
        assert_eq!(
            catalog
                .export_path(&["os", "clock"])
                .and_then(|export| export.return_kind),
            Some(StdlibReturnKind::Float)
        );
        assert_eq!(
            catalog
                .export_path(&["string", "to_float"])
                .and_then(|export| export.return_kind),
            Some(StdlibReturnKind::Float)
        );
        assert_eq!(
            catalog
                .export_path(&["io", "std", "read_to_string"])
                .and_then(|export| export.return_kind),
            Some(StdlibReturnKind::String)
        );
        for global in &catalog.globals {
            if global.lowering_key.is_some() {
                assert!(
                    global.return_kind.is_some(),
                    "global {} has lowering key without return kind",
                    global.name
                );
            }
        }
        for module in &catalog.modules {
            for export in &module.exports {
                assert_lowered_export_has_return_kind(&module.name, export);
            }
        }
    }

    fn assert_lowered_export_has_return_kind(path: &str, export: &crate::StdlibExportSpec) {
        let path = format!("{path}.{}", export.name);
        if export.kind == StdlibExportKind::Function && export.lowering_key.is_some() {
            assert!(
                export.return_kind.is_some(),
                "export {path} has lowering key without return kind"
            );
        }
        for child in &export.children {
            assert_lowered_export_has_return_kind(&path, child);
        }
    }

    #[test]
    fn test_fs_path_env_process_modules() -> Result<()> {
        let mut td = std::env::temp_dir();
        td.push(format!("lk_stdlib_modules_{}", std::process::id()));
        std::fs::create_dir_all(&td)?;
        let mut file = td.clone();
        file.push("data.txt");
        writeln!(File::create(&file)?, "hello")?;
        let cleanup_dir = td.clone();
        let file = file.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");
        let td = td.to_string_lossy().replace('\\', "\\\\").replace('"', "\\\"");

        let src = format!(
            r#"
            use fs;
            use path;
            use env;
            use process;
            let file = "{}";
            if (!(fs.exists(file)
                && fs.is_file(file)
                && fs.read_to_string(file).contains("hello")
                && path.file_name(file) == "data.txt"
                && path.extension(file) == "txt"
                && env.get_or("LK_TEST_ENV_SHOULD_NOT_EXIST_42", "dflt") == "dflt"
                && process.id() > 0)) {{
                return [];
            }}
            return fs.read_dir("{}");
            "#,
            file, td
        );
        let out = run(&src)?;
        let TypedList::String(entries) = runtime_list(out.first_return(), out.state.heap()) else {
            panic!("expected string list");
        };
        assert!(entries.iter().any(|entry| entry.as_ref() == "data.txt"));

        let _ = std::fs::remove_dir_all(cleanup_dir);
        Ok(())
    }

    #[test]
    fn test_encoding_hash_regex_random_uuid_modules() -> Result<()> {
        let out = run(r#"
            use encoding;
            use hash;
            use regex;
            use random;
            use uuid;
            use bytes;
            let parsed = encoding.json.parse("{\"answer\":42}");
            let id = uuid.v4();
            return parsed.answer == 42
                && encoding.hex.encode("hi") == "6869"
                && bytes.to_string_utf8(encoding.hex.decode("6869")) == "hi"
                && hash.sha256("abc") == "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
                && regex.is_match("[0-9]+", "a12")
                && regex.find("[0-9]+", "a12").text == "12"
                && random.int(1, 3) >= 1
                && random.int(1, 3) <= 3
                && uuid.is_valid(id)
                && encoding.url.query_parse("a=1&b=two").b == "two";
            "#)?;
        assert_eq!(out.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    #[test]
    fn test_top_level_json_yaml_toml_are_removed() {
        assert!(run("use json; return json.parse(\"{}\");").is_err());
        assert!(run("use yaml; return yaml.parse(\"a: 1\");").is_err());
        assert!(run("use toml; return toml.parse(\"a = 1\");").is_err());
    }
}
