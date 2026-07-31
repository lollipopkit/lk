#[cfg(test)]
mod tests {
    use lk_core::vm::ModuleResolver;
    use lk_core::vm::ProgramExec;
    use std::{fs::File, io::Write, sync::Arc};

    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        val::{CallableValue, HeapValue, RuntimeVal, TypedList},
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
        // `string.to_float` answers `Float?` — text that is not a number is
        // nil — so its return kind is the boxed one, not `Float`.
        assert_eq!(
            catalog
                .export_path(&["string", "to_float"])
                .and_then(|export| export.return_kind),
            Some(StdlibReturnKind::RuntimeValue)
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
    fn test_macro_generated_metadata_includes_hover_docs_and_nested_children() {
        let catalog = stdlib_catalog();
        let env = catalog.module("env").expect("env module");
        assert_eq!(env.docs.as_deref(), Some("Environment variable helpers"));

        let get = catalog.export_path(&["env", "get"]).expect("env.get export");
        assert_eq!(get.signature.as_deref(), Some("env.get(key: String) -> String?"));
        assert_eq!(
            get.docs.as_deref(),
            Some("Returns an environment variable, or nil if it is not set.")
        );

        let path_join = catalog.export_path(&["path", "join"]).expect("path.join export");
        assert_eq!(
            path_join.signature.as_deref(),
            Some("path.join(first: String, ...rest: String) -> String")
        );

        let bytes_utf8 = catalog
            .export_path(&["bytes", "to_string_utf8"])
            .expect("bytes.to_string_utf8 export");
        assert_eq!(
            bytes_utf8.docs.as_deref(),
            Some("Decodes bytes as UTF-8 and raises an error for invalid input.")
        );

        let encoding = catalog.module("encoding").expect("encoding module");
        assert_eq!(encoding.docs.as_deref(), Some("Encoding and data format helpers"));
        let json = encoding.export("json").expect("encoding.json namespace");
        assert_eq!(json.kind, StdlibExportKind::Module);
        assert!(json.children.iter().any(|child| child.name == "parse"));
        let json_parse = catalog
            .export_path(&["encoding", "json", "parse"])
            .expect("encoding.json.parse export");
        assert_eq!(
            json_parse.signature.as_deref(),
            Some("encoding.json.parse(source: String) -> Value")
        );

        let string_char = catalog.export_path(&["string", "get"]).expect("string.get export");
        assert_eq!(
            string_char.signature.as_deref(),
            Some("string.get(text: String, index: Int) -> String?")
        );
        let string_byte = catalog
            .export_path(&["string", "byte_at"])
            .expect("string.byte_at export");
        assert_eq!(
            string_byte.signature.as_deref(),
            Some("string.byte_at(text: String, index: Int) -> Int?")
        );
        let string_pad_left = catalog
            .export_path(&["string", "pad_left"])
            .expect("string.pad_left export");
        assert_eq!(
            string_pad_left.signature.as_deref(),
            Some("string.pad_left(text: String, width: Int, pad?: String) -> String")
        );
        let string_replace = catalog
            .export_path(&["string", "replace"])
            .expect("string.replace export");
        assert_eq!(
            string_replace.signature.as_deref(),
            Some("string.replace(text: String, pattern: String, with: String, all?: Bool = true) -> String")
        );
        let time_since = catalog.export_path(&["time", "since"]).expect("time.since export");
        assert_eq!(
            time_since.signature.as_deref(),
            Some("time.since(start_ms: Int | Float, end_ms: Int | Float) -> Int")
        );
        let string_split = catalog.export_path(&["string", "split"]).expect("string.split export");
        assert_eq!(
            string_split.signature.as_deref(),
            // Angle brackets, because that is how the language spells a generic:
            // a signature shown on hover has to be one the reader can write down.
            Some("string.split(text: String, separator: String) -> List<String>")
        );
        let stream_collect = catalog
            .export_path(&["stream", "collect"])
            .expect("stream.collect export");
        assert_eq!(
            stream_collect.signature.as_deref(),
            Some("stream.collect(cursor: Stream | Cursor, limit?: Int) -> List")
        );
    }

    /// A module name that is also a global builtin is a dead end, not a style
    /// question: `use chan;` binds the name to the module, so the global
    /// `chan(3)` stops being a call — and until `chan.new` existed there was no
    /// way left to make a channel at all.
    ///
    /// `chan` was the only one. Checked rather than remembered, because the
    /// next module to collide would fail the same silent way: its constructor
    /// would keep working right up until someone imported it.
    #[test]
    fn no_stdlib_module_shadows_a_global_builtin() -> Result<()> {
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;
        crate::register_stdlib_core_globals(&mut registry);
        crate::register_stdlib_concurrency_globals(&mut registry);

        for name in crate::stdlib_module_names() {
            // `chan` is the known exception, and it is *complete*: the module
            // carries `chan.new`, so importing it does not take the constructor
            // away — it renames it. A new collision has no such answer.
            if name == "chan" {
                assert!(
                    registry.get_runtime_builtin("chan::new").is_some(),
                    "chan shadows the global constructor, so the module must carry `new`"
                );
                continue;
            }
            assert!(
                registry.get_runtime_builtin(name).is_none(),
                "module `{name}` is also a global builtin: importing it would shadow the global \
                 and leave whatever the global did unreachable"
            );
        }
        Ok(())
    }

    #[test]
    fn test_stdlib_export_macro_registers_selected_runtime_builtins() -> Result<()> {
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry)?;

        for (name, arity) in [
            ("time::sleep", 1),
            ("time::since", 2),
            ("chan::try_send", 2),
            ("task::join_all", lk_core::vm::NativeEntry::VARIADIC),
        ] {
            let export = registry.get_runtime_builtin(name).expect("runtime builtin");
            let state = export.state_lock().expect("runtime export state lock");
            let RuntimeVal::Obj(handle) = export.value() else {
                panic!("{name} should be heap callable");
            };
            let Some(HeapValue::Callable(CallableValue::RuntimeNative {
                arity: actual_arity, ..
            })) = state.heap().get(*handle)
            else {
                panic!("{name} should use RuntimeNative");
            };
            assert_eq!(*actual_arity, arity, "{name} arity");
        }

        assert!(
            registry.get_runtime_builtin("slice::from_string").is_none(),
            "runtime_builtins=false modules should not register module::function builtins"
        );
        Ok(())
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

    /// `parse` had no `stringify`, so a script could read a config and change
    /// it but not write it back — `base64`, `hex` and `url` next door are all
    /// pairs. Object keys come out sorted (`serde_json::Map` is a `BTreeMap`),
    /// which makes a generated config byte-stable and therefore diffable.
    /// `..` cancels a *named* component and nothing else.
    ///
    /// Two bugs met here. Above a root, a `..` that could not be popped was
    /// pushed back, so `/../a` normalized to `/../a` — a path that normalizes
    /// to itself forever, and one no filesystem agrees with (`/..` is `/`).
    /// And a `..` popped whatever was last, including another `..`, so
    /// `../..` — two levels up — answered the empty string.
    /// `i64::abs` panics on `Int::MIN` — there is no positive one — so
    /// `math.abs` on that single value took the process down, which a script
    /// cannot catch. Wrapping is the language's own rule for Int overflow, and
    /// this *is* an Int overflow.
    #[test]
    fn test_math_abs_of_the_smallest_int_wraps_instead_of_aborting() -> Result<()> {
        let out = run(r#"
            use math;
            let smallest = -9223372036854775807 - 1;
            return [math.abs(smallest), math.abs(-5), math.abs(5)];
            "#)?;
        let list = runtime_list(out.first_return(), out.state.heap());
        let TypedList::Int(values) = list else {
            panic!("expected a list of ints, got {list:?}");
        };
        assert_eq!(values, &[i64::MIN, 5, 5]);
        Ok(())
    }

    #[test]
    fn test_path_normalize_cancels_only_named_components() -> Result<()> {
        let out = run(r#"
            use path;
            return [
                path.normalize("/../a"),
                path.normalize("/a/../.."),
                path.normalize("/a/../../b"),
                path.normalize(".."),
                path.normalize("../.."),
                path.normalize("a/../../b"),
                path.normalize("./a/./b"),
                path.normalize("a/b/../c"),
            ];
            "#)?;
        let list = runtime_list(out.first_return(), out.state.heap());
        let TypedList::String(values) = list else {
            panic!("expected a list of strings, got {list:?}");
        };
        assert_eq!(
            values.iter().map(|value| value.as_ref()).collect::<Vec<_>>(),
            ["/a", "/", "/b", "..", "../..", "../b", "a/b", "a/c"]
        );
        Ok(())
    }

    #[test]
    fn test_encoding_stringify_round_trips_and_refuses_what_json_cannot_spell() -> Result<()> {
        let out = run(r#"
            use encoding;
            let text = encoding.json.stringify({"b": [1, 2], "a": "x"});
            let back = encoding.json.parse(text);
            let refused_key = try {
                let m = {};
                m[1] = 2;
                encoding.json.stringify(m);
                "not refused"
            } catch e { e };
            let refused_set = try { encoding.json.stringify(Set([1])); "not refused" } catch e { e };
            return text == "{\"a\":\"x\",\"b\":[1,2]}"
                && back.a == "x"
                && back.b[1] == 2
                && encoding.json.stringify([1, "a", true, nil]) == "[1,\"a\",true,null]"
                && refused_key.contains("is an Int")
                && refused_set.contains("no JSON form")
                && encoding.yaml.stringify({"a": 1}).contains("a: 1")
                && encoding.toml.stringify({"a": 1}).contains("a = 1");
            "#)?;
        assert_eq!(out.first_return(), &RuntimeVal::Bool(true));
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
                // `regex.find` is declared `Map?` — it finds nothing for some
                // inputs — so the field access has to go through `?.`.
                && regex.find("[0-9]+", "a12")?.text == "12"
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

    /// Type texts the checker is allowed not to understand.
    ///
    /// Every one of these names a runtime handle the type system has no variant
    /// for, so widening it to `Any` is the honest answer. A text that reaches
    /// `Any` *without* being on this list is a declaration the checker silently
    /// gave up on — a typo, or a spelling the alias table has not been taught —
    /// and the export it belongs to would be untyped for no stated reason.
    const UNDERSTOOD_AS_ANY: &[&str] = &[
        "Any", "Bytes", "Resource", "Stream", "Cursor", "Slice", "Value", "Fn", "Task", "Channel",
    ];

    fn is_understood(text: &str) -> bool {
        let text = text.trim().trim_end_matches('?').trim();
        if UNDERSTOOD_AS_ANY.contains(&text) {
            return true;
        }
        if lk_core::typ::type_from_text(text) != lk_core::val::Type::Any {
            return true;
        }
        // A union is understood when each arm is: `Bytes | String` resolves to
        // `Any` as a whole precisely *because* one arm is opaque.
        text.contains('|') && text.split('|').all(is_understood)
    }

    #[test]
    fn every_declared_stdlib_type_is_understood_by_the_checker() {
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry).expect("register stdlib modules");

        let mut unknown: Vec<String> = Vec::new();
        for name in crate::STDLIB_MODULES.iter().map(|entry| entry.name) {
            let Some(metadata) = crate::registered_stdlib_module_metadata(name) else {
                continue;
            };
            for signature in metadata.signatures {
                for param in signature.params {
                    if !is_understood(param.ty) {
                        unknown.push(format!("{}({}: {})", signature.path, param.name, param.ty));
                    }
                }
                if !is_understood(signature.returns) {
                    unknown.push(format!("{} -> {}", signature.path, signature.returns));
                }
            }
        }
        unknown.sort();

        assert!(
            unknown.is_empty(),
            "stdlib declares types the checker cannot act on:\n  {}",
            unknown.join("\n  ")
        );
    }

    /// A parameter whose *position* cannot say what it means must be named.
    ///
    /// The mechanical half of the convention in `docs/stdlib.md`: two
    /// parameters of the same type, past the first, are indistinguishable at
    /// the call site, so swapping them is silent — the program keeps running
    /// and answers something else.
    ///
    /// The evidence this is not hypothetical: `"abcdef".substring(2, 3)` is
    /// `"cde"` (the third argument is a *length*) while
    /// `[1,2,3,4,5,6].slice(2, 3)` is `[3]` (an *end*). Two sibling operations,
    /// identical call sites, different meanings. Only the declaration knows,
    /// and only a name carries the declaration to where the code is read.
    ///
    /// The first parameter is exempt: the subject of a call is what the call is
    /// about, and its position says so. `string.len(text: s)` would be noise.
    #[test]
    fn every_ambiguous_parameter_is_named() {
        let mut registry = ModuleRegistry::new();
        register_stdlib_modules(&mut registry).expect("register stdlib modules");

        let mut unnamed: Vec<String> = Vec::new();
        for name in crate::STDLIB_MODULES.iter().map(|entry| entry.name) {
            let Some(metadata) = crate::registered_stdlib_module_metadata(name) else {
                continue;
            };
            for signature in metadata.signatures {
                // Past the first: the subject is identified by being first.
                let tail = &signature.params[signature.params.len().min(1)..];
                for (index, param) in tail.iter().enumerate() {
                    let shares_type = tail
                        .iter()
                        .enumerate()
                        .any(|(other, candidate)| other != index && candidate.ty == param.ty);
                    if shares_type && !param.named {
                        unnamed.push(format!("{}({}: {})", signature.path, param.name, param.ty));
                    }
                }
            }
        }
        unnamed.sort();
        unnamed.dedup();

        assert!(
            unnamed.is_empty(),
            "these parameters share a type with a sibling and cannot be told apart by position; \
             declare them `named(...)` (docs/stdlib.md):\n  {}",
            unnamed.join("\n  ")
        );
    }
}
