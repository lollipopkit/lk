//! v2 error-model semantics for channels: failures RAISE (catchable with
//! try/catch), success returns the value directly — no `[ok, value]` pairs.
//! `chan.try_recv` returns nil when empty, pairing with postfix `!`.

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
}
