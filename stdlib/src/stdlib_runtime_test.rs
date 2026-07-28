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
        val::{HeapValue, RuntimeVal, TypedList},
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

    /// Every string operation that has both a method and a module spelling,
    /// asserted equal on the same inputs.
    ///
    /// The module form is not a second implementation but it is a second
    /// *declaration*, and the two had drifted five ways before this test
    /// existed — `len` counted bytes on one side and characters on the other,
    /// `find` answered -1 versus nil, `chars` built a differently-typed list,
    /// `substring`'s third parameter was documented as `end` while it is a
    /// length, and `byte_at` was called `byte` here and answered -1 there.
    /// Every one of them was found by comparing, not by reading.
    #[test]
    fn the_string_module_is_a_spelling_of_the_string_methods() -> Result<()> {
        let source = r#"
            use string;
            // Empty, ASCII, multi-byte, padded, and one with separators — the
            // shapes that told the two forms apart.
            let inputs = ["", "a", "abc", "héllo wörld", "  pad  ", "aXbXc"];
            let mismatch = [];
            for s in inputs {
                if (s.len() != string.len(s)) { mismatch.push("len"); }
                if (s.is_empty() != string.is_empty(s)) { mismatch.push("is_empty"); }
                if (s.lower() != string.lower(s)) { mismatch.push("lower"); }
                if (s.upper() != string.upper(s)) { mismatch.push("upper"); }
                if (s.trim() != string.trim(s)) { mismatch.push("trim"); }
                if (s.reverse() != string.reverse(s)) { mismatch.push("reverse"); }
                if (s.chars() != string.chars(s)) { mismatch.push("chars"); }
                if (s.split("X") != string.split(s, "X")) { mismatch.push("split"); }
                if (s.contains("b") != string.contains(s, "b")) { mismatch.push("contains"); }
                if (s.starts_with("a") != string.starts_with(s, "a")) { mismatch.push("starts_with"); }
                if (s.ends_with("c") != string.ends_with(s, "c")) { mismatch.push("ends_with"); }
                if (s.find("b") != string.find(s, "b")) { mismatch.push("find"); }
                if (s.find("zz") != string.find(s, "zz")) { mismatch.push("find-miss"); }
                if (s.repeat(2) != string.repeat(s, 2)) { mismatch.push("repeat"); }
                if (s.substring(1, 2) != string.substring(s, 1, 2)) { mismatch.push("substring"); }
                if (s.replace("X", "-") != string.replace(s, "X", "-")) { mismatch.push("replace"); }
                if (s.byte_at(0) != string.byte_at(s, 0)) { mismatch.push("byte_at"); }
                if (s.byte_at(99) != string.byte_at(s, 99)) { mismatch.push("byte_at-oob"); }
            }
            return mismatch;
        "#;
        let result = run(source)?;
        let RuntimeVal::Obj(handle) = result.first_return() else {
            panic!("expected the mismatch list");
        };
        let names: Vec<String> = match result.state.heap().get(*handle) {
            Some(HeapValue::List(TypedList::String(values))) => values.iter().map(|v| v.to_string()).collect(),
            Some(HeapValue::List(TypedList::Mixed(values))) if values.is_empty() => Vec::new(),
            other => panic!("expected a string list, got {other:?}"),
        };
        assert!(names.is_empty(), "the two spellings disagree on: {}", names.join(", "));
        Ok(())
    }

    /// Absence is nil, including at the byte level.
    ///
    /// `s.byte_at(oob)` answered `-1` — a sentinel, in a language that says nil
    /// everywhere else it means absent (`find`, `get`, `first`, `last`, `pop`),
    /// and against the `Int?` the method itself declares.
    #[test]
    fn an_out_of_range_byte_is_nil_not_a_sentinel() -> Result<()> {
        let result = run(r#"use string;
            return "abc".byte_at(9) == nil && string.byte_at("abc", 9) == nil && "abc".byte_at(0) == 97;"#)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    /// The three sequence types answer the read operations the same way.
    ///
    /// They did not: `Bytes` had no methods at all — `b[0]` was "not
    /// indexable", `for x in b` was a type error — so reading bytes meant
    /// `bytes.to_list(b)`, a copy that also turns each byte into an eight-byte
    /// `Int`. The only way to read bytes was to stop having bytes. `Slice` was
    /// half-way: indexable and iterable, but without `first`/`last`/
    /// `contains`/`index_of`.
    ///
    /// What belongs here is the operations whose meaning does not depend on the
    /// element type. `map` deliberately does not: it cannot answer a `Bytes`,
    /// because a callback may return something that is not a byte.
    #[test]
    fn the_three_sequences_read_alike() -> Result<()> {
        let source = r#"
            let xs = [97, 98, 99];
            let w = xs.slice(0, 3);
            let b = "abc".bytes();
            return xs.len() == 3 && w.len() == 3 && b.len() == 3
                && xs[0] == 97 && w[0] == 97 && b[0] == 97
                && xs[-1] == 99 && w[-1] == 99 && b[-1] == 99
                && xs[9] == nil && w[9] == nil && b[9] == nil
                && xs.first() == 97 && w.first() == 97 && b.first() == 97
                && xs.last() == 99 && w.last() == 99 && b.last() == 99
                && xs.get(1) == 98 && w.get(1) == 98 && b.get(1) == 98
                && xs.contains(98) && w.contains(98) && b.contains(98)
                && xs.index_of(99) == 2 && w.index_of(99) == 2 && b.index_of(99) == 2
                && xs.index_of(1) == 0 - 1 && w.index_of(1) == 0 - 1 && b.index_of(1) == 0 - 1
                && !xs.is_empty() && !w.is_empty() && !b.is_empty()
                && w.to_list() == xs && b.to_list() == xs;
        "#;
        let result = run(source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    /// …including `for`, which is the operation the question started from.
    #[test]
    fn all_three_sequences_iterate() -> Result<()> {
        let source = r#"
            let xs = [97, 98, 99];
            let sums = [];
            for source in [xs, xs.slice(0, 3), "abc".bytes()] {
                let total = 0;
                for value in source { total = total + value; }
                sums.push(total);
            }
            return sums;
        "#;
        let result = run(source)?;
        let RuntimeVal::Obj(handle) = result.first_return() else {
            panic!("expected the sums list");
        };
        let sums = match result.state.heap().get(*handle) {
            Some(HeapValue::List(TypedList::Int(values))) => values.clone(),
            other => panic!("expected an int list, got {other:?}"),
        };
        assert_eq!(sums, vec![294, 294, 294]);
        Ok(())
    }

    /// Which transforms keep a sequence's type, and which cannot.
    ///
    /// The rule is whether the result's elements can be something the receiver
    /// could not hold. `filter` keeps a subset, so a filtered `Bytes` is still
    /// `Bytes` and a `take` of a window is still a window — contiguous, so it
    /// costs nothing. `map` may answer anything, so it is a list whatever it
    /// started from; and `filter` on a *window* is a list too, because what it
    /// keeps is not contiguous.
    #[test]
    fn a_transform_keeps_the_sequence_type_only_when_its_elements_must_fit() -> Result<()> {
        let source = r#"
            use iter;
            let b = "abc".bytes();
            let w = [1, 2, 3, 4].slice(1, 4);
            return b.map(|x| x + 1) == [98, 99, 100]
                && w.map(|x| x * 10) == [20, 30, 40]
                && b.reduce(0, |a, x| a + x) == 294
                && w.reduce(0, |a, x| a + x) == 9
                // `filter` on bytes is bytes: comparing to a list would be
                // comparing two different types.
                && b.filter(|x| x > 97).to_list() == [98, 99]
                && b.take(2).to_list() == [97, 98]
                && b.skip(2).to_list() == [99]
                // …and on a window it is a list, because what it keeps has
                // holes in it.
                && w.filter(|x| x > 2) == [3, 4]
                && w.take(2).to_list() == [2, 3]
                && w.skip(2).to_list() == [4]
                // The module spelling reaches all three for the exports whose
                // result does not depend on which sequence came in.
                && iter.map(b, |x| x + 1) == [98, 99, 100]
                && iter.reduce(w, 0, |a, x| a + x) == 9
                && iter.next(b) == 97;
        "#;
        let result = run(source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    /// Searching a list compares values, not handles.
    ///
    /// It compared handles, and the boundary that drew was `ShortStr`'s
    /// seven-byte inline limit — invisible in the source and decisive in the
    /// answer:
    ///
    /// ```text
    /// ["ab", "cd"].contains("ab")              → true
    /// ["abcdefghij", …].contains("abcdefghij") → false
    /// ```
    ///
    /// Same shape as the `TypedList::String` read bug, in a different method.
    /// A list, a map or a set could never be found at all, at any length.
    #[test]
    fn a_list_is_searched_by_value_not_by_handle() -> Result<()> {
        let source = r#"
            let long = ["abcdefghij", "klmnopqrst"];
            let nested = [[1], [2]];
            let maps = [{"a": 1}, {"b": 2}];
            return long.contains("abcdefghij")
                && long.index_of("klmnopqrst") == 1
                && ["ab", "cd"].contains("ab")
                && nested.contains([1])
                && nested.index_of([2]) == 1
                && maps.contains({"b": 2})
                && ["a", "a", "abcdefghij", "abcdefghij"].unique().len() == 2;
        "#;
        let result = run(source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }

    /// A window equals what it holds.
    ///
    /// It had no equality arm at all, so it fell through to `false`: a window
    /// printed `[97,98,99]` and compared unequal to `[97,98,99]` — and unequal
    /// to another window over the same range of the same list.
    #[test]
    fn a_window_equals_the_elements_it_windows() -> Result<()> {
        let source = r#"
            let xs = [97, 98, 99];
            let w = xs.slice(0, 3);
            return w == xs
                && xs == w
                && w == xs.slice(0, 3)
                && w != xs.slice(0, 2)
                && xs.slice(1, 3) == [98, 99]
                && ["abcdefghij", "x"].slice(0, 1) == ["abcdefghij"];
        "#;
        let result = run(source)?;
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true));
        Ok(())
    }
}
