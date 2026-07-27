#[cfg(test)]
mod tests {
    #[cfg(not(feature = "std"))]
    use crate::compat::prelude::*;
    use crate::{
        typ::{ObservedBinding, TypeChecker},
        val::Type,
    };

    fn observe(src: &str) -> Vec<ObservedBinding> {
        let program = crate::syntax::parse_program_source(src, Default::default()).expect("parse program");
        let mut checker = TypeChecker::new();
        checker.observe_bindings();
        program.type_check_collecting(&mut checker);
        checker.take_observations()
    }

    fn type_of<'a>(observations: &'a [ObservedBinding], name: &str) -> Option<&'a Type> {
        observations
            .iter()
            .find(|binding| binding.name == name)
            .map(|binding| &binding.ty)
    }

    #[test]
    fn bindings_are_recorded_with_their_inferred_types() {
        let observed = observe("let a = 1; let b = \"x\"; let c = 1.5;");

        assert_eq!(type_of(&observed, "a"), Some(&Type::Int));
        assert_eq!(type_of(&observed, "b"), Some(&Type::String));
        assert_eq!(type_of(&observed, "c"), Some(&Type::Float));
    }

    #[test]
    fn a_binding_that_reads_an_earlier_one_is_recorded_too() {
        // The whole point of recording during a program-wide check: `y` has a
        // type only because `x` is in scope, which no per-expression inference
        // in a fresh checker can know.
        let observed = observe("let x = 2; let y = x + 1;");

        assert_eq!(type_of(&observed, "y"), Some(&Type::Int));
    }

    #[test]
    fn bindings_inside_a_function_body_are_recorded() {
        // Recorded while the body's scope is live. A traversal afterwards would
        // find the scope popped and `total` gone with it.
        let observed = observe("fn f(n: Int) -> Int { let total = n * 2; return total; }");

        assert_eq!(type_of(&observed, "total"), Some(&Type::Int));
    }

    #[test]
    fn a_binding_carries_the_span_of_its_statement() {
        let observed = observe("let a = 1;\nlet b = 2;");

        let b = observed.iter().find(|binding| binding.name == "b").expect("b recorded");
        assert_eq!(b.span.start.line, 2, "second let is on line 2");
    }

    #[test]
    fn each_name_of_a_destructuring_pattern_gets_its_own_type() {
        let observed = observe("let [n, s] = [1, \"x\"];");

        assert_eq!(type_of(&observed, "n"), Some(&Type::Int));
        assert_eq!(type_of(&observed, "s"), Some(&Type::String));
    }

    #[test]
    fn collecting_reports_every_bad_statement_not_just_the_first() {
        let program = crate::syntax::parse_program_source("let a: Int = \"x\"; let b: Bool = 1;", Default::default())
            .expect("parse program");
        let mut checker = TypeChecker::new();

        let errors = program.type_check_collecting(&mut checker);

        assert_eq!(
            errors.len(),
            2,
            "both mismatches should be reported: {:?}",
            errors.iter().map(ToString::to_string).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_failed_statement_does_not_cost_the_next_one_its_types() {
        // What error recovery buys an editor: one mistyped line used to take
        // the type hints for every line below it.
        let observed = observe("let bad: Int = \"x\"; let good = 7;");

        assert_eq!(type_of(&observed, "good"), Some(&Type::Int));
    }

    #[test]
    fn nothing_is_recorded_unless_asked_for() {
        let program = crate::syntax::parse_program_source("let a = 1;", Default::default()).expect("parse program");
        let mut checker = TypeChecker::new();

        program.type_check_collecting(&mut checker);

        assert!(checker.take_observations().is_empty());
    }
}
