#[cfg(test)]
mod tests {
    #[cfg(not(feature = "std"))]
    use crate::compat::prelude::*;
    use crate::{ast::Parser as ExprParser, token::Tokenizer, typ::TypeChecker, val::Type};

    /// Type-checks a whole program, as `lk check` does.
    fn check_program(src: &str) -> anyhow::Result<()> {
        let program = crate::syntax::parse_program_source(src, Default::default()).expect("parse program");
        let mut tc = TypeChecker::new();
        program.type_check(&mut tc)
    }

    fn infer(src: &str) -> Type {
        let tokens = Tokenizer::tokenize(src).expect("tokenize");
        let expr = ExprParser::new(&tokens).parse().expect("parse expr");
        let mut tc = TypeChecker::new();
        tc.infer_resolved_type(&expr).expect("infer")
    }

    #[test]
    fn test_string_add_concatenation_rules() {
        // String + String => String
        assert_eq!(infer("\"a\" + \"b\""), Type::String);
        // String + Int => String
        assert_eq!(infer("\"a\" + 1"), Type::String);
        // Int + String => String
        assert_eq!(infer("1 + \"b\""), Type::String);
        // Var + String => String (var constrained to String)
        assert_eq!(infer("x + \"!\""), Type::String);
    }

    /// A literal with mixed element types is a `Tuple`, not a `List` of the
    /// union.
    ///
    /// This test asserted the union until it was mounted and run for the first
    /// time (it, and four other files, had never been compiled). The language
    /// moved under it: a literal has a known length and a known type per
    /// position, and keeping both is strictly more information than collapsing
    /// them — `[1, 2.0][0]` is `Int`, where the union would only promise
    /// `Int | Float`.
    #[test]
    fn test_list_literal_with_mixed_elements_is_a_tuple() {
        let ty = infer("[1, 2.0, 3]");
        match ty {
            Type::Tuple(items) => {
                assert_eq!(items, vec![Type::Int, Type::Float, Type::Int], "per-position types");
            }
            _ => panic!("expected Tuple<...>, got {:?}", ty),
        }
    }

    #[test]
    fn test_map_union_value_type() {
        // Map with mixed value types should infer union value type
        let ty = infer("{\"a\": 1, \"b\": 2.0}");
        match ty {
            Type::Map(k, v) => {
                assert_eq!(*k, Type::String);
                match *v {
                    Type::Union(ts) => {
                        assert!(ts.contains(&Type::Int));
                        assert!(ts.contains(&Type::Float));
                    }
                    _ => panic!("expected union value type, got {:?}", *v),
                }
            }
            _ => panic!("expected Map<...>, got {:?}", ty),
        }
    }

    #[test]
    fn test_indexing_a_literal_gives_the_position_type() {
        // A constant index into a tuple gives that position's type exactly.
        assert_eq!(infer("([1, 2.0])[0]"), Type::Int);
        assert_eq!(infer("([1, 2.0])[1]"), Type::Float);
    }

    /// The top level runs in order, so a statement there cannot read a binding
    /// declared below it — it reads nil, and the error that used to surface
    /// was about nil ("Add expected numbers, got Nil and Int"), naming neither
    /// the binding nor the order.
    #[test]
    fn top_level_read_before_definition_is_reported() {
        let err = check_program("const B = A + 4;\nconst A = 1;\n").expect_err("must be reported");
        let message = err.to_string();
        assert!(message.contains("`A` is used before it is defined"), "{message}");
    }

    /// A function body is the opposite case: it runs after the whole top level,
    /// so reading a `const` declared below it is ordinary — and the bare-metal
    /// programs do it throughout.
    #[test]
    fn a_function_body_may_read_a_later_binding() {
        check_program("fn f() { return LATER; }\nconst LATER = 7;\n").expect("a body may read it");
    }

    /// Same for a closure, for the same reason.
    #[test]
    fn a_closure_body_may_read_a_later_binding() {
        check_program("let g = fn() => TAIL;\nconst TAIL = 9;\n").expect("a closure may read it");
    }

    /// The read in a binding's own initializer still counts.
    #[test]
    fn a_binding_may_not_read_itself() {
        let err = check_program("const A = A + 1;\n").expect_err("must be reported");
        assert!(err.to_string().contains("`A` is used before it is defined"), "{err}");
    }

    /// `map`'s element type is the callback's, decided at the call site.
    ///
    /// The table cannot name it — there is no type there to name until someone
    /// passes a function — so it declared `List<Any>` and every `map` result
    /// was unchecked from then on.
    #[test]
    fn map_takes_its_element_type_from_the_callback() {
        assert!(check_program("let xs = [1, 2]; let bad: String = xs.map(|x| x * 2)[0];").is_err());
        assert!(check_program("let xs = [1, 2]; let ok: Int = xs.map(|x| x * 2)[0];").is_ok());
        // Including when the callback changes the type.
        assert!(check_program(r#"let xs = [1, 2]; let bad: Int = xs.map(|x| "n=${x}")[0];"#).is_err());
        assert!(check_program(r#"let xs = [1, 2]; let ok: String = xs.map(|x| "n=${x}")[0];"#).is_ok());
        // And when it is a named function rather than a literal.
        assert!(
            check_program("fn twice(x: Int) -> Int { return x * 2; } let bad: String = [1].map(twice)[0];").is_err()
        );
    }

    /// A callback's parameter is the receiver's element type, known *before*
    /// its body is checked.
    ///
    /// Adding it afterwards as a constraint types the result but checks
    /// nothing: the body was already read with the parameter still free, so a
    /// method that does not exist on the element type went unnoticed.
    #[test]
    fn a_callback_parameter_is_the_element_type_while_its_body_is_checked() {
        assert!(check_program(r#"let ws = ["a"]; let n: Int = ws.map(|s| s.len())[0];"#).is_ok());
        let error =
            check_program(r#"let ws = ["a"]; let bad = ws.map(|s| s.bogus());"#).expect_err("a String has no `bogus`");
        assert!(
            format!("{error:#}").contains("String has no method 'bogus'"),
            "unexpected error: {error:#}"
        );
        // `filter` keeps the element type, so its predicate sees it too.
        assert!(check_program(r#"let ws = ["a"]; let kept: List<String> = ws.filter(|s| s.len() > 0);"#).is_ok());
    }

    /// A template string is a string, closure body included.
    ///
    /// `|x| "n=${x}"` was a syntax error while `|x| "n"` parsed: the token that
    /// starts an interpolated string was missing from the set a closure body
    /// may begin with.
    #[test]
    fn a_closure_body_may_be_a_template_string() {
        assert!(check_program(r#"let f = |x| "n=${x}"; let s: String = f(1);"#).is_ok());
    }
}
