#[cfg(test)]
mod tests {
    use lk_core::vm::ModuleResolver;
    use lk_core::vm::ProgramExec;
    use std::sync::Arc;

    use crate::{
        register_stdlib_globals, register_stdlib_modules, runtime_native::runtime_string_value, string::StringModule,
    };
    use anyhow::Result;
    use lk_core::{
        module::ModuleRegistry,
        stmt::stmt_parser::StmtParser,
        token::Tokenizer,
        val::{HeapStore, HeapValue, RuntimeVal, ShortStr, TypedList},
        vm::{NativeArgs, NativeEntry, NativeFunction, NativeRuntime, ProgramResult, RuntimeModuleState, VmContext},
    };

    fn execute_string(source: &str) -> Result<ProgramResult> {
        let tokens = Tokenizer::tokenize(source)?;
        let mut parser = StmtParser::new(&tokens);
        let program = parser.parse_program()?;

        let mut registry = ModuleRegistry::new();
        // The globals too, not just the modules: `!` desugars to a nil check
        // that raises through `error`, so without them core syntax fails here
        // with "undefined callable" — a harness gap, not a language one.
        register_stdlib_globals(&mut registry);
        register_stdlib_modules(&mut registry)?;
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn string_native(name: &str) -> Result<(u16, NativeFunction)> {
        crate::runtime_native::runtime_native_export(&StringModule::new(), name)
    }

    fn runtime_str<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> Option<&'a str> {
        match value {
            RuntimeVal::ShortStr(value) => Some(value.as_str()),
            RuntimeVal::Obj(handle) => match heap.get(*handle) {
                Some(HeapValue::String(value)) => Some(value.as_ref()),
                _ => None,
            },
            _ => None,
        }
    }

    fn runtime_list<'a>(value: &'a RuntimeVal, heap: &'a HeapStore) -> &'a TypedList {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected runtime list object");
        };
        let Some(HeapValue::List(list)) = heap.get(*handle) else {
            panic!("expected runtime list heap value");
        };
        list
    }

    #[test]
    fn test_string_len() -> Result<()> {
        let result = execute_string("use string; return string.len(\"hello\");")?;
        assert_eq!(result.first_return(), &RuntimeVal::Int(5));

        Ok(())
    }

    #[test]
    fn test_string_lower() -> Result<()> {
        let result = execute_string("use string; return string.lower(\"HELLO\");")?;
        assert_eq!(runtime_str(result.first_return(), result.state.heap()), Some("hello"));

        Ok(())
    }

    #[test]
    fn test_string_method_sugar() -> Result<()> {
        let result = execute_string("return \"hello\".len();")?;
        assert_eq!(result.first_return(), &RuntimeVal::Int(5));
        Ok(())
    }

    #[test]
    fn test_string_functions_use_runtime_native_abi() -> Result<()> {
        for name in [
            "len",
            "lower",
            "upper",
            "trim",
            "starts_with",
            "ends_with",
            "contains",
            "split",
            "join",
            "reverse",
            "repeat",
            "char_at",
            "byte_at",
            "chars",
            "is_empty",
        ] {
            let (arity, function) = string_native(name)?;
            assert!(
                matches!(function, NativeFunction::Plain(_)),
                "{name} should use plain RuntimeNative"
            );
            assert_ne!(
                arity,
                NativeEntry::VARIADIC,
                "{name} should have fixed positional arity"
            );
        }
        // `slice` joins these: a named parameter does not occupy a
        // positional slot, so a call using one supplies fewer arguments than
        // the declaration lists, and a fixed arity would reject it before the
        // export ran (`slice(s, start: 2, end: 5)`).
        for name in ["replace", "index_of", "format", "slice"] {
            let (arity, function) = string_native(name)?;
            assert!(matches!(function, NativeFunction::Plain(_)));
            assert_eq!(arity, NativeEntry::VARIADIC);
        }
        Ok(())
    }

    /// A `named(...)` parameter can be given positionally *or* by name, in any
    /// mixture — and never both.
    ///
    /// The checker used to build a stdlib function's positional list by
    /// *removing* every named-eligible parameter, so a call that mixed the two
    /// spellings was rejected: `bytes.slice(b, 0, end: 2)` was told the
    /// function "expects 1 positional arguments" while `bytes.slice(b, 0, 2)`
    /// was fine. `math.clamp` was the sole exception, by way of a rule in the
    /// checker naming it — which is why it alone behaved.
    #[test]
    fn named_and_positional_spellings_mix_freely() -> Result<()> {
        let source = r#"
            use string;
            use bytes;
            let all_positional = string.slice("hello", 1, 3);
            let all_named = string.slice("hello", start: 1, end: 3);
            let mixed = string.slice("hello", 1, end: 3);
            let sliced = bytes.slice(bytes.from_list([1, 2, 3]), 0, end: 2);
            let window = if sliced.len() == 2 { "two" } else { "wrong" };
            return [all_positional, all_named, mixed, window];
        "#;
        let result = execute_string(source)?;
        let TypedList::String(values) = runtime_list(result.first_return(), result.state.heap()) else {
            panic!("expected typed string list");
        };
        for (index, value) in values[..3].iter().enumerate() {
            assert_eq!(value.as_ref(), "el", "spelling {index} should agree with the others");
        }
        assert_eq!(
            values[3].as_ref(),
            "two",
            "the mixed-spelling byte window should hold two bytes"
        );
        Ok(())
    }

    /// Naming an argument means what passing it positionally means.
    ///
    /// `replace`'s `all` flag used to default to whether the call *spelled*
    /// its arguments by name: `replace("aaa", "a", "b")` replaced every
    /// occurrence and `replace("aaa", pattern: "a", with: "b")` replaced one.
    /// Same arguments, different answer, decided by punctuation. All three
    /// spellings below now agree.
    #[test]
    fn test_string_replace_named_arguments() -> Result<()> {
        let source = r#"
            use string;
            let named = string.replace("lollipop", pattern: "l", with: "x");
            let named_all = string.replace("lollipop", pattern: "l", with: "x", all: true);
            let positional = string.replace("lollipop", "l", "x");
            return [named, named_all, positional];
        "#;
        let result = execute_string(source)?;
        let TypedList::String(values) = runtime_list(result.first_return(), result.state.heap()) else {
            panic!("expected typed string list");
        };
        assert_eq!(
            values.as_slice(),
            &[
                Arc::<str>::from("xoxxipop"),
                Arc::<str>::from("xoxxipop"),
                Arc::<str>::from("xoxxipop")
            ]
        );
        Ok(())
    }

    #[test]
    fn test_string_replace_duplicate_named_argument_error() {
        let mut heap = HeapStore::new();
        let source = runtime_string_value("lol", &mut heap);
        let named_args = [
            RuntimeVal::ShortStr(ShortStr::new("pattern").expect("short")),
            runtime_string_value("l", &mut heap),
            RuntimeVal::ShortStr(ShortStr::new("pattern").expect("short")),
            runtime_string_value("x", &mut heap),
            RuntimeVal::ShortStr(ShortStr::new("with").expect("short")),
            runtime_string_value("a", &mut heap),
        ];
        let (_, function) = string_native("replace").expect("replace native");
        let NativeFunction::Plain(function) = function else {
            panic!("replace should use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::new(heap, Vec::new());
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        let err = function(
            NativeArgs::new_with_named_stack(&[source], &named_args, 0, 3),
            &mut runtime,
        )
        .expect_err("duplicate named arguments should error");
        assert!(err.to_string().contains("duplicate named argument"));
    }

    #[test]
    fn test_string_slice_out_of_range_is_empty() -> Result<()> {
        // Clamped, not an error — the same as everywhere else a position runs
        // past the end in this language: `s[1..99]` answers `"bc"`,
        // `xs[0..99]` answers the whole list, `xs.get(99)` answers nil. This
        // was the module form's own convention (it raised) while the method
        // form clamped, so the two disagreed about the same call.
        let result = execute_string("use string; return string.slice(\"abc\", 10, 11);")?;
        assert_eq!(
            result.first_return(),
            &RuntimeVal::ShortStr(ShortStr::new("").expect("empty"))
        );

        Ok(())
    }

    #[test]
    fn test_method_and_module_forms_agree_on_multibyte_text() -> Result<()> {
        // The two spellings of every string operation had drifted apart:
        // `"héllo wörld".len()` answered 11 (characters) while
        // `string.len(…)` answered 13 (bytes), `find` answered `-1` on one
        // side and `nil` on the other, and `substring` panicked on both when a
        // position landed inside a multi-byte character. They share one
        // implementation now; this is what keeps them sharing it.
        let source = r#"
            use string;
            let s = "héllo wörld";
            return [
                s.len() == string.len(s),
                s.index_of("wörld") == string.index_of(s, "wörld"),
                s.index_of("zz") == string.index_of(s, "zz"),
                s.slice(2, 5) == string.slice(s, 2, 5),
                s.slice(0, s.len()) == s,
                s.slice(s.index_of("wörld")!, s.index_of("wörld")! + 5) == "wörld",
                s.len() == 11,
                s.index_of("zz") == nil,
            ];
        "#;
        let result = execute_string(source)?;
        let TypedList::Bool(values) = runtime_list(result.first_return(), result.state.heap()) else {
            panic!("expected a list of booleans");
        };
        assert!(
            values.iter().all(|holds| *holds),
            "method and module forms disagree: {values:?}"
        );
        Ok(())
    }

    #[test]
    fn test_bytes_is_the_explicit_way_to_byte_positions() -> Result<()> {
        // Characters are the default; bytes are asked for. `s.len()` and
        // `s.bytes().len()` disagree on purpose, and which one you get is now
        // the reader's choice rather than a property of which spelling of the
        // operation they happened to reach for.
        let source = r#"
            use bytes;
            let s = "héllo";
            return [
                s.len() == 5,
                s.bytes().len() == 6,
                s.bytes() == bytes.from_string(s),
                bytes.to_string_utf8(s.bytes()) == s,
                s.chars() == ["h", "é", "l", "l", "o"],
            ];
        "#;
        let result = execute_string(source)?;
        let TypedList::Bool(values) = runtime_list(result.first_return(), result.state.heap()) else {
            panic!("expected a list of booleans");
        };
        assert!(
            values.iter().all(|holds| *holds),
            "byte/character split broke: {values:?}"
        );
        Ok(())
    }

    #[test]
    fn test_string_join_rejects_non_string_items() {
        let source = "use string; return string.join([\"ok\", 123], \",\");";
        let err = execute_string(source).expect_err("non-string list elements should error");
        assert!(err.to_string().contains("list must contain only strings"));
    }

    #[test]
    fn test_string_runtime_direct_call_with_heap_string() -> Result<()> {
        let mut heap = HeapStore::new();
        let input = runtime_string_value("hello", &mut heap);
        let suffix = runtime_string_value("lo", &mut heap);
        let (_, function) = string_native("ends_with")?;
        let NativeFunction::Plain(function) = function else {
            panic!("ends_with should use plain RuntimeNative");
        };
        let mut state = RuntimeModuleState::new(heap, Vec::new());
        let mut runtime = NativeRuntime::new(&mut state, None, None);
        let result = function(NativeArgs::new(&[input, suffix]), &mut runtime)?;
        assert_eq!(result, RuntimeVal::Bool(true));
        Ok(())
    }

    /// Both pad functions measured the width in *bytes* and then sliced the
    /// repeated fill by byte offset, so a multi-byte fill cut inside a
    /// character and **panicked the process** — which a script cannot catch.
    /// Characters is also the unit everything else counts: `s.len()`, `s[i]`,
    /// `s.slice(a, b)`.
    #[test]
    fn pad_counts_characters_and_survives_a_multibyte_fill() -> Result<()> {
        let out = execute_string(
            r#"
            use string;
            return [
                string.pad_left("a", 5, "中"),
                string.pad_right("a", 5, "中"),
                string.pad_left("中文", 4, "-"),
                string.pad_left("a", 5, "xy"),
                string.pad_left("abcdef", 3, "-"),
            ];
            "#,
        )?;
        let TypedList::String(values) = runtime_list(out.first_return(), out.state.heap()) else {
            panic!("expected a list of strings");
        };
        assert_eq!(
            values.iter().map(|value| value.as_ref()).collect::<Vec<_>>(),
            ["中中中中a", "a中中中中", "--中文", "xyxya", "abcdef"]
        );
        Ok(())
    }

    /// `strip`'s parameter has always been named `chars` — a *set* — but the
    /// body stripped the whole string as a prefix, and only if that failed as a
    /// suffix, once: `strip("--a--", "-")` answered `"-a--"`. `strip_prefix`
    /// and `strip_suffix` next door are the once-each operations.
    #[test]
    fn strip_removes_every_leading_and_trailing_character_in_the_set() -> Result<()> {
        let out = execute_string(
            r#"
            use string;
            return [
                string.strip("--a--", "-"),
                string.strip("xxaybyxx", "xy"),
                string.strip("abc", "-"),
                string.strip("---", "-"),
            ];
            "#,
        )?;
        let TypedList::String(values) = runtime_list(out.first_return(), out.state.heap()) else {
            panic!("expected a list of strings");
        };
        assert_eq!(
            values.iter().map(|value| value.as_ref()).collect::<Vec<_>>(),
            ["a", "ayb", "abc", ""]
        );
        Ok(())
    }

    /// Reading a number out of text is the operation LK did not have.
    ///
    /// `string.to_int` looked like the answer and refused a `String` outright,
    /// so a program could split a CSV, read a config or take an argument and
    /// had nowhere to go. Text that is not a number answers `nil` (a question
    /// about input, not a program error); a Float with no Int raises.
    #[test]
    fn to_int_reads_text_and_refuses_a_float_with_no_int() -> Result<()> {
        let out = execute_string(
            r#"
            use string;
            return [
                string.to_int("42") ?? -1,
                string.to_int("  42\n") ?? -1,
                string.to_int("-42") ?? -1,
                string.to_int("42abc") ?? -1,
                string.to_int("") ?? -1,
                string.to_int("42.0") ?? -1,
                string.to_int("9223372036854775808") ?? -1,
                string.to_int("ff", 16) ?? -1,
                string.to_int("-101", 2) ?? -1,
                string.to_int("9", 8) ?? -1,
                string.to_int(3.99) ?? -1,
                string.to_int(-3.99) ?? -1,
                string.to_int(true) ?? -1,
            ];
            "#,
        )?;
        let TypedList::Int(values) = runtime_list(out.first_return(), out.state.heap()) else {
            panic!("expected a list of ints");
        };
        assert_eq!(values, &[42, 42, -42, -1, -1, -1, -1, 255, -5, -1, 3, -3, 1]);

        for (source, expected) in [
            ("string.to_int(0.0 / 0.0);", "NaN"),
            ("string.to_int(1e30);", "outside the Int range"),
            ("string.to_int(\"7\", 1);", "base must be between 2 and 36"),
        ] {
            let error = execute_string(&format!("use string;\n{source}")).expect_err(source);
            assert!(
                format!("{error:#}").contains(expected),
                "`{source}` should mention `{expected}`: {error:#}"
            );
        }
        Ok(())
    }

    /// The `Float` half, including the values only a Float has.
    #[test]
    fn to_float_reads_text_including_nan_and_the_infinities() -> Result<()> {
        let out = execute_string(
            r#"
            use string;
            return [
                string.to_float("3.5") ?? -1.0,
                string.to_float(" -2e3 ") ?? -1.0,
                string.to_float("abc") ?? -1.0,
                string.to_float("") ?? -1.0,
                string.to_float(7) ?? -1.0,
                string.to_float(true) ?? -1.0,
            ];
            "#,
        )?;
        let TypedList::Float(values) = runtime_list(out.first_return(), out.state.heap()) else {
            panic!("expected a list of floats");
        };
        assert_eq!(values, &[3.5, -2000.0, -1.0, -1.0, 7.0, 1.0]);

        let out = execute_string("use string;\nreturn string.to_float(\"inf\");")?;
        assert_eq!(out.first_return(), &RuntimeVal::Float(f64::INFINITY));
        let out = execute_string("use string;\nreturn string.to_float(\"nan\");")?;
        let RuntimeVal::Float(value) = out.first_return() else {
            panic!("expected a float");
        };
        assert!(value.is_nan(), "`nan` parses to NaN, got {value}");
        Ok(())
    }
    /// One convention for a negative position, across every sequence.
    ///
    /// `xs[-1]` and `xs.get(-1)` have always counted from the end. `slice` had
    /// four implementations and three answers: List and Bytes raised, String
    /// and Slice clamped to 0 and returned a window nobody asked for — and the
    /// *native* string slice already counted from the end, so
    /// `"abcde".slice(1, -1)` was `""` interpreted and `"bcd"` compiled.
    #[test]
    fn a_negative_slice_bound_counts_from_the_end_on_every_sequence() -> Result<()> {
        let out = execute_string(
            r#"
            use bytes;
            let s = "abcde";
            let xs = [1, 2, 3, 4, 5];
            let b = bytes.from_string("abcde");
            return [
                s.slice(-2, 5),
                s.slice(1, -1),
                s.slice(-99, 99),
                s.slice(-1, -3),
                "${xs.slice(-2, 5).to_list()}",
                "${xs.slice(1, -1).to_list()}",
                "${xs.slice(-99, 99).to_list()}",
                "${xs.slice(-1, -3).to_list()}",
                "${b.slice(-2, 5)}",
                "${b.slice(1, -1)}",
            ];
            "#,
        )?;
        let TypedList::String(values) = runtime_list(out.first_return(), out.state.heap()) else {
            panic!("expected a list of strings");
        };
        assert_eq!(
            values.iter().map(|value| value.as_ref()).collect::<Vec<_>>(),
            [
                "de",
                "bcd",
                "abcde",
                "",
                "[4,5]",
                "[2,3,4]",
                "[1,2,3,4,5]",
                "[]",
                "Bytes([100,101])",
                "Bytes([98,99,100])",
            ]
        );
        Ok(())
    }

    /// Every `string` module member answers exactly what its method spelling
    /// answers — by construction, because the module forwards.
    ///
    /// It did not, and the divergences were live: `split(s, "")` was
    /// `["a","b","c"]` through the module and `["","a","b","c",""]` through the
    /// method, `slice(s, -1, 3)` raised through the module while the method
    /// counted from the end (the language's own rule for a negative position),
    /// and `byte_at(s, -1)` raised on one side and answered nil on the other.
    /// Fifteen operations had two bodies; three of them had already drifted.
    ///
    /// The comparison is the language's own `==`, on the inputs where the two
    /// used to differ — reading the two answers back out of rendered text was a
    /// second parser to get wrong, and I got it wrong first.
    #[test]
    fn every_module_spelling_answers_what_the_method_answers() -> Result<()> {
        let source = r#"
            use string;
            let s = "abc";
            return [
                string.split(s, "") == s.split(""),
                string.slice(s, -1, 3) == s.slice(-1, 3),
                string.byte_at(s, -1) == s.byte_at(-1),
                string.upper(s) == s.upper(),
                string.len("héllo") == "héllo".len(),
                string.replace("aa", "a", "b", false) == "aa".replace("a", "b", false),
                string.index_of(s, "z") == s.index_of("z"),
                string.repeat(s, 0) == s.repeat(0),
                string.chars(s) == s.chars(),
                string.trim("  a  ") == "  a  ".trim(),
            ];
        "#;
        let result = execute_string(source)?;
        let rendered = lk_core::vm::display_runtime_value(result.first_return(), result.state.heap());
        assert!(
            !rendered.contains("false"),
            "a module spelling and its method disagree: {rendered}"
        );
        Ok(())
    }
}
