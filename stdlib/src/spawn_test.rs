//! LK-source behavior tests for goroutines: `spawn(closure)` (snapshot
//! promote — see `spawnable_callable`) and blocking channel ops called from
//! *inside* a goroutine (`Runtime::block_on`'s `block_in_place` path). The
//! `go` statement is parse-time sugar over the same `spawn`.

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
    fn spawn_runs_a_plain_closure() {
        assert_true(
            r#"
            use task;
            let t = spawn(|| 7);
            return task.await(t) == 7;
            "#,
        );
    }

    #[test]
    fn spawn_snapshots_captured_locals() {
        assert_true(
            r#"
            use task;
            let base = 10;
            let t = spawn(|| base + 32);
            return task.await(t) == 42;
            "#,
        );
    }

    /// Top-level `fn`s are closure values in globals — the snapshot must
    /// carry them, or goroutines couldn't call named functions.
    #[test]
    fn goroutine_can_call_named_functions() {
        assert_true(
            r#"
            use task;
            fn double(x) { return x * 2; }
            let t = spawn(|| double(21));
            return task.await(t) == 42;
            "#,
        );
    }

    /// Channels are Arc-shared (not copied), so a goroutine's blocking
    /// `send` — running on a tokio worker — pairs with the main thread's
    /// `recv`. This is the block_in_place path.
    #[test]
    fn goroutine_blocking_send_pairs_with_main_recv() {
        assert_true(
            r#"
            use task;
            let c = chan(1);
            let t = spawn(|| {
                let i = 0;
                while (i < 3) {
                    send(c, i * 100);
                    i = i + 1;
                }
                return "done";
            });
            let got = [];
            got.push(recv(c)[1]);
            got.push(recv(c)[1]);
            got.push(recv(c)[1]);
            return task.await(t) == "done" && got == [0, 100, 200];
            "#,
        );
    }

    /// Isolate semantics: the goroutine mutates its own snapshot; the
    /// spawner's local is untouched. Communication goes through channels.
    #[test]
    fn goroutine_mutations_do_not_leak_back() {
        assert_true(
            r#"
            use task;
            let counter = 0;
            let t = spawn(|| {
                counter = counter + 1;
                return counter;
            });
            let inside = task.await(t);
            return inside == 1 && counter == 0;
            "#,
        );
    }

    /// Nested closures inside captures survive the same-module structural
    /// copy.
    #[test]
    fn captures_containing_closures_are_copied_structurally() {
        assert_true(
            r#"
            use task;
            let add = |a, b| a + b;
            let t = spawn(|| add(20, 22));
            return task.await(t) == 42;
            "#,
        );
    }

    /// Two goroutines rendezvous through a channel: true parallelism with
    /// CSP-style communication.
    #[test]
    fn two_goroutines_communicate_over_a_channel() {
        assert_true(
            r#"
            use task;
            let c = chan(4);
            let producer = spawn(|| {
                send(c, 1);
                send(c, 2);
                return nil;
            });
            let consumer = spawn(|| {
                let a = recv(c)[1];
                let b = recv(c)[1];
                return a + b;
            });
            task.await(producer);
            return task.await(consumer) == 3;
            "#,
        );
    }

    #[test]
    fn spawn_still_rejects_non_functions() {
        // Caught by the type checker when it can prove it, by the native
        // otherwise — either way it's an error mentioning "function".
        let err = run("spawn(42);").expect_err("non-callable must fail");
        assert!(err.to_string().contains("function"), "{err}");
    }
}
