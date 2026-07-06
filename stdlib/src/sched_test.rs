//! LK-source behavior tests for the `sched` cooperative scheduler — the
//! parser→compiler→coroutine→scheduler integration surface. The Rust-level
//! unit tests for the park/wake internals (await, blocking select, deadlock
//! detection) live in `stdlib/crates/sched`; these exercise what a user
//! actually writes.

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

    /// Producer/consumer over a capacity-1 channel: the producer must park on
    /// the full channel and the consumer must park on the empty one — the
    /// round-robin queue plus the blocking select both get exercised.
    #[test]
    fn producer_consumer_park_and_wake_over_bounded_channel() {
        assert_true(
            r#"
            use sched;
            use chan as ch;
            let c = chan(1);
            let seen = [];
            let producer = || {
                let i = 0;
                while (i < 3) {
                    yield sched.send(c, i);
                    i = i + 1;
                }
                ch.close(c);
                return "done";
            };
            let consumer = || {
                let n = 0;
                while (true) {
                    let r = yield sched.recv(c);
                    if (!r[0]) { break; }
                    seen.push(r[1]);
                    n = n + 1;
                }
                return n;
            };
            let results = sched.run(producer, consumer);
            return results[0] == [true, "done"] && results[1] == [true, 3] && seen == [0, 1, 2];
            "#,
        );
    }

    /// `sched.spawn` with arguments + `sched.join` for the child's result.
    #[test]
    fn spawn_with_args_and_join() {
        assert_true(
            r#"
            use sched;
            let root = || {
                let child = yield sched.spawn(|a, b| {
                    yield sched.sleep(1);
                    return a + b;
                }, 20, 22);
                let joined = yield sched.join(child);
                return joined;
            };
            let r = sched.run(root);
            return r[0] == [true, [true, 42]];
            "#,
        );
    }

    /// One root erroring must not stop the others; its result is `[false,
    /// message]`, mirroring `pcall`/`coroutine_resume`.
    #[test]
    fn coroutine_errors_are_isolated_per_root() {
        assert_true(
            r#"
            use sched;
            let bad = || { error("boom"); };
            let good = || { yield sched.pause(); return 1; };
            let r = sched.run(bad, good);
            return r[0][0] == false && r[0][1] == "boom" && r[1] == [true, 1];
            "#,
        );
    }

    /// `yield` of anything that isn't a scheduler descriptor is a clear,
    /// catchable error (generator-style coroutines belong to bare
    /// `coroutine_resume`, not `sched.run`).
    #[test]
    fn non_descriptor_yield_is_a_catchable_error() {
        assert_true(
            r#"
            use sched;
            let stray = || { yield 42; };
            let caught = pcall(|| sched.run(stray));
            return caught[0] == false;
            "#,
        );
    }

    /// A join cycle is the one provably-stuck shape (nothing external can
    /// complete a join), and must be reported as a deadlock instead of
    /// hanging. Handles are wired through a shared map, with a pause loop
    /// making the test independent of scheduling order.
    #[test]
    fn join_cycles_are_reported_as_deadlock() {
        assert_true(
            r#"
            use sched;
            let m = {"ready": false, "a": nil, "b": nil};
            let waiter = |own, other| {
                while (!m["ready"]) {
                    yield sched.pause();
                }
                return yield sched.join(m[other]);
            };
            let wire = || {
                let ha = yield sched.spawn(waiter, "a", "b");
                let hb = yield sched.spawn(waiter, "b", "a");
                m["a"] = ha;
                m["b"] = hb;
                m["ready"] = true;
                return 0;
            };
            let caught = pcall(|| sched.run(wire));
            return caught[0] == false;
            "#,
        );
    }

    /// Already-created coroutines can be scheduled directly; duplicates are
    /// rejected.
    #[test]
    fn accepts_coroutines_and_rejects_duplicates() {
        assert_true(
            r#"
            use sched;
            fn work() { yield sched.pause(); return 5; }
            let co = coroutine_create(work);
            let r = sched.run(co);
            let dup = coroutine_create(work);
            let caught = pcall(|| sched.run(dup, dup));
            return r[0] == [true, 5] && caught[0] == false;
            "#,
        );
    }

    /// `sched.sleep` parks on a timer without blocking the sibling coroutine:
    /// the pauser keeps getting scheduled while the sleeper waits.
    #[test]
    fn sleep_parks_without_blocking_siblings() {
        assert_true(
            r#"
            use sched;
            let order = [];
            let sleeper = || {
                yield sched.sleep(15);
                order.push("slept");
                return 1;
            };
            let worker = || {
                let i = 0;
                while (i < 3) {
                    yield sched.pause();
                    i = i + 1;
                }
                order.push("worked");
                return 2;
            };
            let r = sched.run(sleeper, worker);
            return r == [[true, 1], [true, 2]] && order == ["worked", "slept"];
            "#,
        );
    }

    /// Joining a coroutine the scheduler doesn't manage is an error, not a
    /// hang.
    #[test]
    fn joining_unmanaged_coroutine_errors() {
        assert_true(
            r#"
            use sched;
            fn idle() { yield sched.pause(); return 0; }
            let outsider = coroutine_create(idle);
            let root = || {
                return yield sched.join(outsider);
            };
            let caught = pcall(|| sched.run(root));
            return caught[0] == false;
            "#,
        );
    }
}
