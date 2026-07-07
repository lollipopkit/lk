//! LK-source behavior tests for the `select` statement, which parses into
//! `select$block` + plain AST (see `core/src/ast/parser.rs`'s
//! `parse_select`). The desugared *shape* is pinned by
//! `core/src/expr/select_guard_parsing_test.rs`; these pin what running it
//! actually does.

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
    fn ready_recv_arm_fires_and_binds_the_value() {
        assert_true(
            r#"
            let c = chan(1);
            send(c, 42);
            let got = select {
                case v <- recv(c) => v + 1;
                default => -1;
            };
            return got == 43;
            "#,
        );
    }

    #[test]
    fn ready_send_arm_fires_and_delivers() {
        assert_true(
            r#"
            let c = chan(1);
            let fired = select {
                case send(c, 7) => "sent";
                default => "skipped";
            };
            let r = recv(c);
            return fired == "sent" && r == 7;
            "#,
        );
    }

    #[test]
    fn default_fires_when_nothing_is_ready() {
        assert_true(
            r#"
            let c = chan(1);
            let got = select {
                case v <- recv(c) => v;
                default => "empty";
            };
            return got == "empty";
            "#,
        );
    }

    #[test]
    fn false_guard_disables_an_arm() {
        assert_true(
            r#"
            let c = chan(1);
            send(c, 1);
            let got = select {
                case v <- recv(c) if false => v;
                default => "guarded-out";
            };
            // The arm was skipped, so the value is still buffered.
            let still_there = recv(c);
            return got == "guarded-out" && still_there == 1;
            "#,
        );
    }

    /// Channel operands, send values, and guards evaluate eagerly, once, in
    /// source order (the Go rule), regardless of which arm fires.
    #[test]
    fn operands_evaluate_eagerly_in_source_order() {
        assert_true(
            r#"
            let c = chan(1);
            send(c, 5);
            let trace = [];
            let pick = |tag, value| {
                trace.push(tag);
                return value;
            };
            let got = select {
                case v <- recv(pick("ch0", c)) if pick("g0", true) => v;
                case send(pick("ch1", c), pick("v1", 9)) if pick("g1", false) => -1;
                default => -2;
            };
            return got == 5 && trace == ["ch0", "g0", "ch1", "v1", "g1"];
            "#,
        );
    }

    /// Case bodies are single expressions (like match arms) evaluated once,
    /// in the enclosing environment — side effects through captured state
    /// land exactly once, and only for the fired arm.
    #[test]
    fn case_bodies_run_once_in_the_enclosing_environment() {
        assert_true(
            r#"
            let c = chan(1);
            send(c, 10);
            let total = 0;
            let add = |v| {
                total = total + v;
                return total;
            };
            let got = select {
                case v <- recv(c) => add(v);
                default => add(1000);
            };
            return got == 10 && total == 10;
            "#,
        );
    }

    /// A blocking select (no default) with a ready arm completes without
    /// hanging — the `has_default = false` path.
    #[test]
    fn blocking_select_with_ready_arm_completes() {
        assert_true(
            r#"
            let c = chan(1);
            send(c, "ready");
            let got = select {
                case v <- recv(c) => v;
            };
            return got == "ready";
            "#,
        );
    }

    /// A closed channel is always ready (Go semantics): its recv arm fires
    /// with a nil binding once the buffer is drained, so consumers can
    /// observe shutdown through select.
    #[test]
    fn select_on_closed_channel_fires_with_nil_binding() {
        assert_true(
            r#"
            use chan as ch;
            let c = chan(1);
            send(c, 9);
            ch.close(c);
            // Buffered value first...
            let first = select {
                case v <- recv(c) => v;
                default => "never";
            };
            // ...then the drained+closed channel stays selectable with nil.
            let second = select {
                case v <- recv(c) => v == nil ? "closed" : "value";
                default => "never";
            };
            return first == 9 && second == "closed";
            "#,
        );
    }

    /// All arms guarded off and no default: evaluates to nil instead of
    /// blocking forever (`select$block` reports the empty-arms case as
    /// default; documented v1 semantics, unlike Go's deadlock panic).
    #[test]
    fn fully_guarded_out_select_without_default_is_nil() {
        assert_true(
            r#"
            let c = chan(1);
            let got = select {
                case v <- recv(c) if false => v;
            };
            return got == nil;
            "#,
        );
    }

    #[test]
    fn nested_select_in_case_body() {
        assert_true(
            r#"
            let outer = chan(1);
            let inner = chan(1);
            send(outer, 1);
            send(inner, 2);
            let got = select {
                case a <- recv(outer) => select {
                    case b <- recv(inner) => a + b;
                    default => -1;
                };
                default => -2;
            };
            return got == 3;
            "#,
        );
    }
}
