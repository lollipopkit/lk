//! v2 error-model semantics for channels: failures RAISE (catchable with
//! try/catch), success returns the value directly — no `[ok, value]` pairs.
//! `chan.try_recv` returns nil when empty, pairing with postfix `!`.

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
        crate::register_stdlib_globals(&mut registry);
        let resolver = Arc::new(ModuleResolver::with_registry(registry));
        let mut env = VmContext::new().with_resolver(resolver);
        program.execute_with_ctx(&mut env)
    }

    fn assert_true(source: &str) {
        let result = run(source).expect("program should run");
        assert_eq!(result.first_return(), &RuntimeVal::Bool(true), "source:\n{source}");
    }

    #[test]
    fn recv_returns_the_value_directly() {
        assert_true(
            r#"
            let c = chan(1);
            send(c, 42);
            return recv(c) == 42;
            "#,
        );
    }

    #[test]
    fn recv_on_closed_channel_raises_catchably() {
        assert_true(
            r#"
            use chan as ch;
            let c = chan(1);
            ch.close(c);
            let caught = "";
            try { recv(c); } catch e { caught = e; }
            // Native raises carry a wrapper prefix; the cause is what matters.
            return caught.contains("receive on closed channel");
            "#,
        );
    }

    #[test]
    fn send_on_closed_channel_raises_catchably() {
        assert_true(
            r#"
            use chan as ch;
            let c = chan(1);
            ch.close(c);
            let caught = false;
            try { send(c, 1); } catch e { caught = true; }
            return caught;
            "#,
        );
    }

    /// `try_recv`: value when ready, nil when empty (not an error) — postfix
    /// `!` turns "must have a value" into an assertion.
    /// The module is usable on its own: `use chan;` shadows the `chan` global,
    /// and the blocking pair used to exist only as unqualified `send`/`recv`,
    /// so an imported channel could only be polled.
    #[test]
    fn the_module_spells_the_blocking_pair_too() {
        assert_true(
            r#"
            use chan;
            let c = chan.new(2);
            chan.send(c, 41);
            chan.send(c, 42);
            return chan.recv(c) == 41 && chan.recv(c) == 42 && chan.len(c) == 0;
            "#,
        );
    }

    /// `0` is unbuffered, not unbounded: one value fits, the second does not.
    /// lkrt read the retired rule (`<= 0` unbounded) and answered `true` twice.
    #[test]
    fn capacity_zero_is_unbuffered_and_negative_raises() {
        assert_true(
            r#"
            use chan;
            let c = chan.new(0);
            let first = chan.try_send(c, 1);
            let second = chan.try_send(c, 2);
            let negative = try { chan.new(-1); false } catch e { true };
            // `capacity` reports what was asked for, not the queue's bound.
            return first && !second && chan.len(c) == 1 && negative && chan.capacity(c) == 0;
            "#,
        );
    }

    #[test]
    fn try_recv_yields_value_or_nil_and_pairs_with_unwrap() {
        assert_true(
            r#"
            use chan as ch;
            let c = chan(2);
            send(c, 5);
            let ready = ch.try_recv(c);
            let empty = ch.try_recv(c);
            let unwrap_caught = false;
            try { ch.try_recv(c)!; } catch e { unwrap_caught = true; }
            return ready == 5 && empty == nil && unwrap_caught;
            "#,
        );
    }

    /// A drain loop's idiom under raise semantics: catch terminates it.
    #[test]
    fn drain_until_closed_with_try_catch() {
        assert_true(
            r#"
            use chan as ch;
            use task;
            let c = chan(4);
            let t = spawn(|| {
                send(c, 1);
                send(c, 2);
                send(c, 3);
                ch.close(c);
                return nil;
            });
            task.await(t);
            let total = 0;
            try {
                while (true) {
                    total = total + recv(c);
                }
            } catch e {
                // closed: loop done
            }
            return total == 6;
            "#,
        );
    }

    /// Calling an imported module says **what it is** and what to do.
    ///
    /// `use chan;` binds the module over the `chan()` global — a documented
    /// sharp edge — and a module is a map of its members, so `chan(1)` calls a
    /// Map. The error used to be "Call callee is not callable": an opcode
    /// operand, with nothing in it to act on. The type checker catches a plain
    /// map (`{"a": 1}(1)` is "Cannot call non-function type"); it does not
    /// model an import's value, so this path is the one a program reaches.
    #[test]
    fn calling_an_imported_module_says_what_it_is() {
        let error = run("use chan;\nlet c = chan(1);\n").expect_err("a module is not callable");
        let text = format!("{error:#}");
        assert!(text.contains("not a function"), "unexpected error: {text}");
        assert!(text.contains("Map"), "the error should name the value's type: {text}");
        assert!(text.contains("module"), "and point at how a program gets here: {text}");
    }

    /// Awaiting twice says so, instead of describing the task table.
    #[test]
    fn awaiting_twice_says_the_result_is_already_taken() {
        let error = run("use task;\nlet h = spawn(|| 5);\nlet a = task.await(h);\nlet b = task.await(h);\n")
            .expect_err("the second await has nothing to take");
        let text = format!("{error:#}");
        assert!(
            text.contains("already been awaited"),
            "the error should speak the language: {text}"
        );
    }
}
