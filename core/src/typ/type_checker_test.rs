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

    /// A `catch` that renders the error is the ordinary shape, and it was the
    /// one shape `try` rejected.
    ///
    /// The two branches of a value were unified — the same type or an error —
    /// while a function with two `return`s of different types has always been a
    /// *union*. So `try { xs.take(1) } catch e { "${e}" }` was "Cannot unify
    /// List<Int> with String", and the caught value renders as text, so that is
    /// what a `catch` most often evaluates to.
    ///
    /// The union is the answer, not a shrug: an annotation still refuses it,
    /// naming both halves.
    #[test]
    fn a_try_whose_branches_differ_is_a_union() {
        check_program("let xs = [1, 2, 3];\nprintln(try { xs.take(1) } catch e { \"${e}\" });\n")
            .expect("a rendered catch is the ordinary shape");

        let error = check_program("let xs = [1, 2, 3];\nlet a: Int = try { xs } catch e { \"${e}\" };\n")
            .expect_err("the union does not fit an Int");
        let message = format!("{error:#}");
        assert!(
            message.contains("List<Int> | String"),
            "the annotation should be told both halves: {message}"
        );

        // Same type on both sides stays that type, and a branch that answers
        // nothing still makes the value optional.
        assert_eq!(infer("try { 1 } catch e { 2 }"), Type::Int);
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

    /// A call to an unannotated function gets *this call's* answer for the
    /// callee's type variables.
    ///
    /// `fn id(x) { return x; }` has the principal type `'a -> 'a`, and every
    /// call site used to constrain that same `'a` — so two calls with different
    /// types fought over it and the result was a type nothing could be checked
    /// against. `let s: String = id(1);` passed.
    #[test]
    fn a_call_reads_the_callees_type_variables_for_itself() {
        assert!(check_program("fn id(x) { return x; } let bad: String = id(1);").is_err());
        assert!(check_program("fn id(x) { return x; } let ok: Int = id(1);").is_ok());
        // Two calls at different types, both right, neither deciding for the
        // other.
        assert!(check_program(r#"fn id(x) { return x; } let a: Int = id(1); let b: String = id("s");"#).is_ok());
        // A parameter that appears in the return type carries through it.
        assert!(check_program(r#"fn pair(x) { return [x, x]; } let bad: Int = pair("s")[0];"#).is_err());
        assert!(check_program(r#"fn pair(x) { return [x, x]; } let ok: String = pair("s")[0];"#).is_ok());
    }

    /// The limit of the above, written down because it is not obvious from
    /// either side.
    ///
    /// `fn first(xs) { return xs[0]; }` is `List<'a> -> 'a`, and a call to it
    /// still learns nothing: what a call site is handed in program mode is the
    /// *placeholder* signature registered before the body was checked — `'b ->
    /// 'c`, with the relation between them living only in the solver. Binding
    /// `'b` to `List<Int>` there says nothing about `'c`.
    ///
    /// Registering the solved signature instead was tried and is worse: it
    /// resolves the placeholders against the body alone, which loses the cases
    /// that do work today (`id`, `pair`) and costs a corpus example besides.
    /// Getting this one needs the call site to reach the solver, which is a
    /// different design than the single pass this checker is.
    #[test]
    fn a_parameter_that_only_shapes_the_return_type_is_not_carried_through_yet() {
        assert!(check_program("fn first(xs) { return xs[0]; } let bad: String = first([1, 2]);").is_ok());
    }

    /// …but not into a union, which describes several map keys at once.
    ///
    /// `{"name": name, "score": 95}` is `Map<String, 'a | Int>`: the union is
    /// every key's value type run together, so no single read is decided by it.
    /// Pinning `'a` to `String` there does not make `u.score` more knowable, it
    /// makes `u.score + 5` — which runs fine — report "left side must be
    /// numeric, got String | Int".
    #[test]
    fn instantiation_stops_at_a_union() {
        assert!(
            check_program(
                r#"fn user(name) { return {"name": name, "score": 95}; }
                   let u = user("Alice");
                   let n = u.score + 5;"#
            )
            .is_ok()
        );
    }

    /// A builtin container dispatches with its element type erased — a
    /// `TypedList::Mixed` has nothing else to report — so `List<Int>` and
    /// `List<String>` reach the same entry. Naming one is refused rather than
    /// registered under a key nothing looks up, which is what used to happen:
    /// the call then failed with "List has no method", true and unhelpful.
    #[test]
    fn an_impl_target_cannot_name_an_element_type() {
        assert!(check_program("impl List { fn second(self) -> Any { return self.get(1); } }").is_ok());
        assert!(check_program("impl Map { fn size(self) -> Int { return self.len(); } }").is_ok());

        let error = check_program("impl List<Int> { fn total(self) -> Int { return 0; } }")
            .expect_err("`List<Int>` is not a dispatchable target");
        assert!(
            format!("{error:#}").contains("cannot name an element type"),
            "unexpected error: {error:#}"
        );
    }

    /// The other half: a method registered on `List` is found on a list of any
    /// element type. It was registered under `List<Any>` and looked up under
    /// `List<Int>`, so it existed and could not be found.
    #[test]
    fn a_method_on_a_builtin_container_is_found_whatever_its_elements_are() {
        assert!(
            check_program(
                "impl List { fn second(self) -> Any { return self.get(1); } }\n\
                 let a = [1, 2].second();\n\
                 let b = [\"x\", \"y\"].second();"
            )
            .is_ok()
        );
    }

    /// A nullable value does not pass for a non-nullable one — including for
    /// numbers, where the promotion rule used to erase the `?`.
    ///
    /// `index_of` answers `Int?` because a miss is nil. Assigning that to a
    /// declared `Int` was accepted, so the nil arrived at whatever read the
    /// variable next and failed there — a report about the wrong line, for a
    /// mistake the annotation was written to catch.
    #[test]
    fn an_optional_number_is_not_a_number() {
        let error = check_program("let n: Int = [1, 2].index_of(2);").expect_err("`Int?` is not an `Int`");
        assert!(
            format!("{error:#}").contains("Int?"),
            "the error should name the nullable type: {error:#}"
        );
        // The same rule the non-numeric types always had.
        assert!(check_program("let s: String = [\"a\"].index_of(\"a\");").is_err());
        // And the ways to say "I have handled the nil" still work.
        assert!(check_program("let n: Int = [1, 2].index_of(2)!;").is_ok());
        assert!(check_program("let n: Int = [1, 2].index_of(2) ?? 0;").is_ok());
        assert!(check_program("let n: Int? = [1, 2].index_of(2);").is_ok());
        assert!(check_program("let n = [1, 2].index_of(2);").is_ok());
    }

    /// A function-type annotation flows **into** a lambda.
    ///
    /// Checked in isolation a lambda types as `('T0) -> Any`, which does not
    /// unify with the annotation written for it — so a lambda could not be
    /// annotated at all, while a named `fn` assigned to the same binding was
    /// accepted.
    #[test]
    fn a_lambda_can_be_annotated_with_a_function_type() {
        assert!(check_program("let f: (Int) -> Int = |x| { return x + 1; };").is_ok());
        assert!(check_program("let g: (Int, Int) -> Int = |a, b| a * b;").is_ok());
        assert!(check_program("let h: (String) -> Int = |s| { return s.len(); };").is_ok());
        assert!(check_program("let n: () -> String = || { return \"hi\"; };").is_ok());
        // A named function was always accepted; it must stay so.
        assert!(check_program("fn inc(x: Int) -> Int { return x + 1; }\nlet f: (Int) -> Int = inc;").is_ok());
        // The annotation still has to be satisfied.
        assert!(check_program("let bad: (Int) -> String = |x| { return x + 1; };").is_err());
        assert!(check_program("let bad: (Int, Int) -> Int = |x| { return x; };").is_err());
    }

    /// A lambda's own parameter and return types are writable.
    ///
    /// It was the one callable in the language whose types could not be written
    /// down — `Type::Function` has always had both halves, so a lambda's
    /// parameter type could only be *guessed* from a call site.
    #[test]
    fn a_lambda_can_declare_its_own_types() {
        assert!(check_program("let f = |x: Int| { return x + 1; };").is_ok());
        assert!(check_program("let g = |a: Int, b: Int| -> Int { return a * b; };").is_ok());
        assert!(check_program("let h = |s: String| -> Int { return s.len(); };").is_ok());
        assert!(check_program("let k = |x| -> Int { return x + 1; };").is_ok());
        assert!(check_program("let m = |xs: List<Int>| -> Int { return xs.len(); };").is_ok());
        // A comma inside the type does not end the parameter.
        assert!(check_program("let n = |m: Map<String, Int>, k: String| -> Int { return m.len(); };").is_ok());
        // The declared return type is checked against, not merely recorded.
        assert!(check_program("let bad = |x: Int| -> String { return x + 1; };").is_err());
        // And a declared parameter type is what the body is checked with.
        assert!(check_program("let bad = |s: String| { return s + 1; };\nlet n: Int = bad(\"a\");").is_err());
    }

    /// Every context that declares a function type accepts a lambda for it.
    ///
    /// A `let` was taught; a struct field was not, so `Handler { run: |x| …  }`
    /// against `run: (Int) -> Int` was rejected with "got `('T0) -> Any`".
    /// One helper now answers for all of them.
    #[test]
    fn a_declared_function_type_accepts_a_lambda_anywhere() {
        assert!(check_program("let f: (Int) -> Int = |x| { return x + 1; };").is_ok());
        assert!(
            check_program(
                "struct Handler { run: (Int) -> Int }\nlet h = Handler { run: |x| { return x * 2; } };\nlet n: Int = h.run(4);"
            )
            .is_ok()
        );
        // And the declaration is still enforced there.
        assert!(
            check_program("struct Handler { run: (Int) -> String }\nlet h = Handler { run: |x| { return x * 2; } };")
                .is_err()
        );
    }

    /// Every position that declares a function type — not just the two that
    /// were taught first.
    ///
    /// Each context had to be taught separately, so each that was not silently
    /// rejected the lambda written for it: a `fn` parameter said "got `('T1) ->
    /// Int`", a declared return type said "Return type mismatch", and a
    /// `List<(Int) -> Int>` annotation refused a list of lambdas.
    #[test]
    fn a_lambda_reaches_every_position_that_declares_its_type() {
        // A parameter.
        assert!(
            check_program("fn apply(g: (Int) -> Int, v: Int) -> Int { return g(v); }\nlet n: Int = apply(|x| { return x + 1; }, 5);")
                .is_ok()
        );
        // A declared return type.
        assert!(check_program("fn make() -> (Int) -> Int { return |x| { return x + 1; }; }").is_ok());
        // An aggregate's element or value type.
        assert!(check_program("let fs: List<(Int) -> Int> = [|x| { return x + 1; }];").is_ok());
        assert!(check_program("let m: Map<String, (Int) -> Int> = {\"a\": |x| { return x + 1; }};").is_ok());
        // Assignment to an already-declared binding.
        assert!(check_program("let f: (Int) -> Int = |x| { return x + 1; };\nf = |x| { return x * 2; };").is_ok());

        // Every one of them still *checks*: the expectation flowing in is half
        // of it, the answer is checked against the declaration too.
        assert!(check_program("fn apply(g: (Int) -> String, v: Int) -> String { return g(v); }\nlet s = apply(|x| { return x + 1; }, 5);").is_err());
        assert!(check_program("fn make() -> (Int) -> String { return |x| { return x + 1; }; }").is_err());
        assert!(check_program("let fs: List<(Int) -> String> = [|x| { return x + 1; }];").is_err());
        assert!(check_program("let m: Map<String, (Int) -> String> = {\"a\": |x| { return x + 1; }};").is_err());
        // A mismatched arity is not a lambda this expectation applies to.
        assert!(check_program("fn apply(g: (Int, Int) -> Int, v: Int) -> Int { return g(v, v); }\nlet n = apply(|x| { return x + 1; }, 5);").is_err());
    }

    /// One name, one meaning: an `impl` may not redefine a method, nor take a
    /// field's name.
    ///
    /// Both were resolved by taking the last declaration, silently. The field
    /// case was worse than that: which one `p.get(…)` meant depended on the
    /// *argument count* — `p.get()` read the field (the method unreachable),
    /// while `p.f(3)` called the method (the field's closure unreachable).
    #[test]
    fn a_method_name_is_declared_once() {
        // The same method twice for one type, whether through one trait…
        assert!(
            check_program(
                "trait Show { fn show(self) -> String; }\nstruct P { x: Int }\nimpl Show for P { fn show(self) -> String { return \"a\"; } }\nimpl Show for P { fn show(self) -> String { return \"b\"; } }"
            )
            .is_err()
        );
        // …two different traits (there is no `Trait::method(x)` to disambiguate
        // with, so `p.run()` would have no answer)…
        assert!(
            check_program(
                "trait A { fn run(self) -> Int; }\ntrait B { fn run(self) -> Int; }\nstruct P { x: Int }\nimpl A for P { fn run(self) -> Int { return 1; } }\nimpl B for P { fn run(self) -> Int { return 2; } }"
            )
            .is_err()
        );
        // …or two inherent blocks.
        assert!(
            check_program("struct P { x: Int }\nimpl P { fn get(self) -> Int { return 1; } }\nimpl P { fn get(self) -> Int { return 2; } }")
                .is_err()
        );
        // A method named like a field, in both arities.
        assert!(check_program("struct P { get: Int }\nimpl P { fn get(self) -> Int { return 9; } }").is_err());
        assert!(check_program("struct P { f: (Int) -> Int }\nimpl P { fn f(self) -> Int { return 9; } }").is_err());

        // Distinct names on one type, and one name on distinct types, are fine.
        assert!(
            check_program(
                "struct P { x: Int }\nimpl P { fn get(self) -> Int { return 1; } fn set(self) -> Int { return 2; } }"
            )
            .is_ok()
        );
        assert!(
            check_program("struct P { x: Int }\nstruct Q { x: Int }\nimpl P { fn get(self) -> Int { return 1; } }\nimpl Q { fn get(self) -> Int { return 2; } }")
                .is_ok()
        );
    }

    /// A trait's required methods are checked where `lk check` can see them.
    ///
    /// The check existed and only ran when the *VM* registered impls, so the
    /// pre-flight command passed a program that could not run.
    #[test]
    fn a_missing_trait_method_is_a_check_error() {
        let error = check_program(
            "trait Show { fn show(self) -> String; fn tag(self) -> Int; }\nstruct P { x: Int }\nimpl Show for P { fn show(self) -> String { return \"a\"; } }",
        )
        .expect_err("`tag` is missing");
        assert!(format!("{error:#}").contains("required by trait"), "{error:#}");

        // A trait *default* is copied into the impl before this runs, so
        // omitting a defaulted method is not an omission.
        assert!(
            check_program(
                "trait Greet { fn hi(self) -> String { return \"hi\"; } }\nstruct P { x: Int }\nimpl Greet for P {}"
            )
            .is_ok()
        );
    }

    /// An arm an earlier catch-all shadows can never run.
    ///
    /// You wrote a case you believe happens, and it does not — silently, with
    /// nothing ever saying the branch is dead. Refused rather than warned about
    /// because the checker has no warning channel, and a loud refusal is what
    /// the language does elsewhere for the same shape of mistake.
    #[test]
    fn a_match_arm_after_a_catch_all_is_refused() {
        for source in [
            "let n = 1;\nlet r = match n { _ => \"any\", 1 => \"one\" };",
            // A *binding* is a catch-all too — this is the one that was sitting
            // in `examples/syntax/unsupported.lk`.
            "let r = match 99 { n => n, _ => 0 };",
            // So is an or-pattern with a total alternative.
            "let n = 1;\nlet r = match n { 1 | _ => \"a\", 2 => \"b\" };",
        ] {
            let error = check_program(source).expect_err(&alloc::format!("dead arm accepted:\n{source}"));
            assert!(format!("{error:#}").contains("can never run"), "{error:#}");
        }

        // A *guarded* catch-all is conditional, so it dominates nothing — the
        // same distinction the fall-through detection draws.
        assert!(check_program("let n = 1;\nlet r = match n { x if x > 0 => \"pos\", _ => \"other\" };").is_ok());
        // And a catch-all as the last arm is the ordinary shape.
        assert!(check_program("let n = 1;\nlet r = match n { 1 => \"one\", _ => \"other\" };").is_ok());
        // A destructuring pattern matches only some values.
        assert!(check_program("let pt = [10, 20];\nlet r = match pt { [x, y] => x + y, _ => 0 };").is_ok());
    }

    /// A top-level `let` may not take a name a declaration already binds.
    ///
    /// A `fn` and a type declaration are hoisted — mutual recursion works, so a
    /// `fn` is visible before its line — and the `let` won *in either order*,
    /// silently. That is the mistake that put a dead `fn apply` beside a live
    /// `let apply` in `examples/syntax/closure.lk`.
    #[test]
    fn a_top_level_let_cannot_take_a_declared_name() {
        for source in [
            "fn pick() -> String { return \"fn\"; }\nlet pick = || { return \"let\"; };",
            // Also the other way round: order does not make it coherent.
            "let pick = || { return \"let\"; };\nfn pick() -> String { return \"fn\"; }",
            "fn pick() -> String { return \"fn\"; }\nlet pick = 42;",
            "struct P { x: Int }\nlet P = 1;",
            "type Alias = Int;\nlet Alias = 1;",
        ] {
            let error = check_program(source).expect_err(&alloc::format!("collision accepted:\n{source}"));
            let text = alloc::format!("{error:#}");
            assert!(text.contains("is already declared as a"), "{text}");
        }

        // Two `let`s *are* coherent shadowing: both are order-sensitive.
        assert!(check_program("let x = 1;\nlet x = 2;").is_ok());
        // And inside a callable body it is ordinary shadowing — the local is
        // order-sensitive within its scope, the declaration is outside it.
        assert!(
            check_program("fn pick() -> Int { return 1; }\nfn use_it() -> Int { let pick = 2; return pick; }").is_ok()
        );
        assert!(check_program("fn pick() -> Int { return 1; }\nlet f = || { let pick = 2; return pick; };").is_ok());
    }

    /// A `fn` inside another callable is parsed and then not found by the
    /// compiler (function indices come from top-level statements only), so it
    /// used to fail with "Compiler undefined function" — the backend's words for
    /// a construct the grammar accepted.
    #[test]
    fn a_function_cannot_be_declared_inside_another() {
        let error =
            check_program("fn outer() -> Int {\n  fn helper(n: Int) -> Int { return n + 1; }\n  return helper(5);\n}")
                .expect_err("a nested fn is refused");
        let text = format!("{error:#}");
        assert!(text.contains("cannot be declared inside another"), "{text}");
        // The message names both ways to say it instead.
        assert!(text.contains("top level") && text.contains("closure"), "{text}");
        // A closure body is a callable body too.
        assert!(check_program("let f = || { fn helper() -> Int { return 1; } return helper(); };").is_err());
        // Top level, including mutual recursion, is unaffected — and so are
        // `impl` methods, which are `fn` declarations at top level.
        assert!(
            check_program(
                "fn is_even(n: Int) -> Bool { if (n == 0) { return true; } return is_odd(n - 1); }\nfn is_odd(n: Int) -> Bool { if (n == 0) { return false; } return is_even(n - 1); }"
            )
            .is_ok()
        );
        assert!(check_program("struct P { x: Int }\nimpl P {\n  fn get(self) -> Int { return self.x; }\n}").is_ok());
    }

    /// A block-bodied closure's `return` is what the closure returns.
    ///
    /// The frame collecting them was popped and discarded, so every such
    /// closure typed `… -> Any` — and `Any` satisfies anything.
    #[test]
    fn a_closure_returns_what_its_body_returns() {
        assert!(check_program("let f = |x| { return x + 1; };\nlet s: String = f(1);").is_err());
        assert!(check_program("let f = |x| { return x + 1; };\nlet n: Int = f(1);").is_ok());
    }

    /// Statements inside a block are type-checked.
    ///
    /// `Expr::Block` used to answer `Any` without looking inside, on the grounds
    /// that blocks mostly come from desugars checked before they are built. But
    /// a closure body is a block too, so a whole class of code was invisible:
    /// this exact `let` is rejected at top level and was accepted here.
    #[test]
    fn a_block_body_is_type_checked() {
        assert!(check_program("let s: String = 1;").is_err());
        assert!(check_program("let f = |x| { let s: String = 1; return x; };").is_err());
        assert!(check_program("let f = |x| { let s: String = \"ok\"; return x; };").is_ok());
        // A block is a scope: the inner binding is not visible afterwards.
        assert!(check_program("let f = || { let inner = 1; return inner; };\nlet n: Int = f();").is_ok());
    }
    /// A key that can never be a key is refused where it is written.
    ///
    /// Only nil, Bool, Int and String can be a map key or a set member — Float
    /// because `0.0 == -0.0` while their bits differ and NaN is not equal to
    /// itself, containers because a key you can mutate is a record you can no
    /// longer find (`docs/semantics.md`). The runtime enforced it from one
    /// place; the checker enforced it from none, so `Set([1.5])` and
    /// `{1.5: "a"}` type-checked and raised at run time with the offending type
    /// sitting in the literal.
    ///
    /// Four sites ask, because there are four ways to write a key: `Set(xs)`,
    /// a map literal, `s.add(v)`, and `m[k] = v` — the last of which the checker
    /// could not even see, since the parser turns it into `__lk_set_index` and
    /// only the bytecode compiler knew that name.
    #[test]
    fn a_type_that_can_never_be_a_key_is_refused_at_check_time() {
        for source in [
            "let s = Set([1.5]);",
            "let s = Set([[1]]);",
            // A tuple names its elements one by one, so one bad element settles it.
            "let s = Set([1, 1.0]);",
            r#"let m = {1.5: "a"};"#,
            r#"let m = {1: "a", 2.5: "b"};"#,
            "let s = Set();\ns.add(1.5);",
            "let s = Set();\ns.contains([1]);",
            r#"let m = {"k": 1};
               m[1.5] = 2;"#,
        ] {
            let error = check_program(source).expect_err(source);
            let message = format!("{error:#}");
            assert!(
                message.contains("cannot be a map key") || message.contains("cannot be a set member"),
                "{source} → {message}"
            );
        }
    }

    /// …and nothing else is refused.
    ///
    /// The rule answers "certainly not a key", not "not obviously a key": a
    /// union may be the Int at run time, a list index is an ordinary position,
    /// and a receiver of unknown type is nobody's business to refuse. Rejecting
    /// a working program is the failure mode that matters here.
    #[test]
    fn the_key_rule_refuses_nothing_that_might_work() {
        for source in [
            "let s = Set([1, 2]);",
            r#"let s = Set([nil, true, "x"]);"#,
            r#"let m = {"k": 1};"#,
            "let m = {1: 2, true: 3};",
            // A list index is a position, not a key.
            "let xs = [1, 2];\nxs[0] = 9;",
            "let i = 0;\nlet xs = [1];\nxs[i] = 5;",
            // Values are unrestricted — only keys are.
            r#"let m = {"k": 1.5};"#,
            r#"let m = {"k": [1, 2]};"#,
            "let s = Set([1]);\nlet v = s.values();",
        ] {
            assert!(check_program(source).is_ok(), "{source}");
        }
    }

    /// An argument's type error says *where*, like every other type error.
    ///
    /// `TypeError::span` is filled by the enclosing statement on the way out, and a
    /// bare call is a `Stmt::Expr` — the one statement variant that carried no span
    /// at all. So the same mistake reported `1:1-6` when written as a `let` and
    /// nothing when written as a call, which in a four-thousand-line program is the
    /// difference between a diagnostic and a riddle.
    ///
    /// Pinned on the *fifth* statement on purpose: an expression carries no position
    /// of its own, so the tempting repair is to search the token stream for
    /// something that looks like it — which finds the first match in the file, not
    /// this one.
    #[test]
    fn an_argument_type_error_names_the_line_it_is_on() {
        let error = check_program(
            r#"fn f(a: Int) -> Int { return a; }
               f(1);
               f(2);
               f(3);
               f("wrong");"#,
        )
        .expect_err("a String is not an Int");

        let message = format!("{error:#}");
        assert!(message.contains("Argument 1 has the wrong type"), "{message}");
        assert!(
            message.contains("(5:"),
            "the error must point at the fifth line: {message}"
        );
    }

    /// `Tuple<A, B>` describes a list, and a list satisfies it — in both
    /// directions.
    ///
    /// `Tuple<Int, Int>` used to be a type nothing could inhabit: `[1, 2]` is
    /// `List<Int>` (its elements do not differ, so no tuple is inferred) and
    /// `is_assignable_to` had only the Tuple→List half. `Tuple<Int, String>`
    /// hid it, because a heterogeneous literal infers `Tuple` directly and
    /// never needed the conversion.
    ///
    /// The unifier had both directions in one arm all along, so this was also
    /// the two of them disagreeing.
    /// A struct literal names a type, and a name nothing declares is refused.
    ///
    /// It used to be accepted "as a named type": `Nope { a: 1 }` answered
    /// `Nope{a:1}` — a typo that produced a value. The checker's own comment
    /// said so ("otherwise, accept as named type"), which is the whole of the
    /// rule it was following.
    #[test]
    fn a_struct_literal_names_a_declared_type() {
        check_program("let p = Nope { a: 1 };").expect_err("nothing declares Nope");
        check_program("struct P { x: Int }\nlet p = P { x: 1 };").expect("declared here");
        // Order does not matter: a top-level declaration is visible before the
        // line it is written on, the same rule `let` shadowing follows.
        check_program("let p = P { x: 1 };\nstruct P { x: Int }").expect("declared later");
        // The message says where a type from another module is reached.
        let message = check_program("let p = Nope { a: 1 };")
            .expect_err("refused")
            .to_string();
        assert!(message.contains("no type named `Nope` is declared here"), "{message}");
        assert!(message.contains("m.Nope"), "{message}");
    }

    /// A type another module declares is not constructible by its bare name.
    ///
    /// It used to build a value that renders `P{x:4}` and answers `typeof` `P`
    /// while carrying none of `P`'s methods: the runtime stamps the
    /// *constructing* module's `TypeScope` and the method table is keyed by the
    /// declaring one, so the two are different identities that share a name.
    /// The failure surfaced at the call site — "P has no method 'norm'" — far
    /// from the construction. The spellings that do carry the declaring
    /// module's identity (`m.P { … }`, or a constructor it exports) both work.
    #[test]
    fn a_type_from_another_module_is_not_constructible_by_its_bare_name() {
        let imported = || crate::typ::StructDef {
            name: "P".to_string(),
            fields: [("x".to_string(), Type::Int)].into_iter().collect(),
        };

        let mut checker = TypeChecker::new();
        checker.registry_mut().register_imported_struct(imported());
        let program =
            crate::syntax::parse_program_source("let p = P { x: 4 };", Default::default()).expect("parse program");
        let message = program
            .type_check(&mut checker)
            .expect_err("a bare imported name is refused")
            .to_string();
        assert!(message.contains("is declared in another module"), "{message}");
        assert!(message.contains("m.P"), "{message}");

        // A local declaration of the same name wins — imports are seeded first,
        // and registering the local one un-marks the entry.
        let mut checker = TypeChecker::new();
        checker.registry_mut().register_imported_struct(imported());
        let program =
            crate::syntax::parse_program_source("struct P { x: Int }\nlet p = P { x: 4 };", Default::default())
                .expect("parse program");
        program
            .type_check(&mut checker)
            .expect("declared here, so it is this module's");

        // Imported *by name*, the literal is accepted: the import binds the
        // constructor the declaring module generates, so the object it builds
        // carries that module's identity. Its schema is still the declaring
        // module's — an undeclared field is refused.
        let mut checker = TypeChecker::new();
        checker.registry_mut().register_imported_struct(imported());
        checker.registry_mut().mark_constructible_import("P", "P");
        let program =
            crate::syntax::parse_program_source("let p = P { x: 4 };", Default::default()).expect("parse program");
        program.type_check(&mut checker).expect("imported by name, so bound");

        let mut checker = TypeChecker::new();
        checker.registry_mut().register_imported_struct(imported());
        checker.registry_mut().mark_constructible_import("P", "P");
        let program =
            crate::syntax::parse_program_source("let p = P { y: 4 };", Default::default()).expect("parse program");
        let message = program
            .type_check(&mut checker)
            .expect_err("the declaring module's schema still applies")
            .to_string();
        assert!(message.contains("Unknown field 'y'"), "{message}");

        // Under an alias the two names differ: the literal is written `Q`, and
        // everything about it — schema, error wording, result type — is `P`'s.
        let mut checker = TypeChecker::new();
        checker.registry_mut().register_imported_struct(imported());
        checker.registry_mut().mark_constructible_import("Q", "P");
        let program =
            crate::syntax::parse_program_source("let q: P = Q { x: 4 };", Default::default()).expect("parse program");
        program.type_check(&mut checker).expect("an alias for an imported type");

        let mut checker = TypeChecker::new();
        checker.registry_mut().register_imported_struct(imported());
        checker.registry_mut().mark_constructible_import("Q", "P");
        let program =
            crate::syntax::parse_program_source("let q = Q { y: 4 };", Default::default()).expect("parse program");
        let message = program
            .type_check(&mut checker)
            .expect_err("the declaring module's schema still applies")
            .to_string();
        assert!(message.contains("struct 'P'"), "{message}");
    }

    /// A top-level statement whose call reaches a binding declared below it.
    ///
    /// The top level runs in order, so `f()` above `const LATER` read nil —
    /// and `typeof(f())` answered `Nil` for a function declared `-> Int`.
    /// Whatever touched the nil next reported its own complaint; nothing ever
    /// named the ordering. Python raises `NameError` here and JavaScript raises
    /// out of the temporal dead zone.
    ///
    /// The other two cases were already settled: a direct top-level read is
    /// refused, and a *body* reading a later binding is ordinary because bodies
    /// run after the whole top level.
    #[test]
    fn a_top_level_call_may_not_reach_a_binding_declared_below_it() {
        let message = check_program("fn f() -> Int { return LATER; }\nprintln(f());\nconst LATER = 7;\n")
            .expect_err("`f` runs before line 3 does")
            .to_string();
        assert!(message.contains("`f` reads `LATER`"), "{message}");
        assert!(message.contains("the top level runs in order"), "{message}");

        // Transitively, through a second function.
        let message = check_program(
            "fn inner() -> Int { return LATER; }\n\
             fn outer() -> Int { return inner(); }\n\
             println(outer());\n\
             const LATER = 7;\n",
        )
        .expect_err("the read is one call deeper")
        .to_string();
        assert!(message.contains("`LATER`"), "{message}");

        // Below the declaration it is ordinary, and so is a body that reads a
        // later binding without being called yet.
        check_program("fn f() -> Int { return LATER; }\nconst LATER = 7;\nprintln(f());\n")
            .expect("the declaration runs first");
        check_program("fn f() -> Int { return LATER; }\nconst LATER = 7;\n").expect("never called above it");

        // A local of the same name is not the global one (shadowing is
        // subtracted wholesale — see `stmt::init_order`).
        check_program("fn f() -> Int { let LATER = 1; return LATER; }\nprintln(f());\nconst LATER = 7;\n")
            .expect("the read is the local");
    }

    /// A method a scalar does not have is a *check-time* error, like it already
    /// was on a String or a List.
    ///
    /// Int, Float, Bool and Nil have **no** built-in methods at all — `abs`,
    /// `sqrt`, `round`, `len`, `to_string`, every one of them answers "no
    /// method" at run time. But `receiver_kind` only knows the six container
    /// kinds, so a scalar receiver fell through to `Any` and `lk check` passed
    /// `let v = 1; v.nope();` — the same mistake, caught at check time on a
    /// `String` and at run time on an `Int`.
    ///
    /// A Map stays exempt on purpose: its entries *are* its fields, so
    /// `m.score(1)` may be an ordinary property call and nothing in the type
    /// says which keys exist.
    #[test]
    fn a_method_a_scalar_does_not_have_is_refused_at_check_time() {
        for (receiver, name) in [("1", "Int"), ("1.5", "Float"), ("true", "Bool"), ("nil", "Nil")] {
            let message = check_program(&alloc::format!("let v = {receiver};\nprintln(v.nope());\n"))
                .expect_err("a scalar has no methods of its own")
                .to_string();
            assert!(
                message.contains(&alloc::format!("{name} has no method 'nope'")),
                "{message}"
            );
        }

        // A user `impl` is what makes the name resolvable, and it does so
        // wherever it sits — including below the call.
        check_program("impl Int { fn double(self) -> Int { return self * 2; } }\nprintln((5).double());\n")
            .expect("an impl above the call");
        check_program("println((5).double());\nimpl Int { fn double(self) -> Int { return self * 2; } }\n")
            .expect("an impl below the call");

        // A map keeps answering: its keys are not in its type.
        check_program("let m = {\"a\": 1};\nlet f = m.whatever();\n").expect("a map's entries are its fields");
    }

    /// `impl` is hoisted like `fn` and `struct`, so a method call may stand
    /// above the block that declares it.
    ///
    /// It was the one declaration form the checker read in source order: a
    /// method becomes known by being type-*checked*, and that walk is ordered.
    /// The imported twin was already pre-scanned (`typ::imports`), which is how
    /// the asymmetry hid — an impl one `use` away worked, one three lines down
    /// did not.
    #[test]
    fn an_impl_is_visible_above_the_block_that_declares_it() {
        check_program(
            "struct P { x: Int }\n\
             let p = P { x: 1 };\n\
             println(p.m());\n\
             impl P { fn m(self) -> Int { return self.x; } }\n",
        )
        .expect("an impl below its call site");

        // Hoisting makes it *visible*, not unchecked: the declared arity still
        // applies from above.
        let message = check_program(
            "struct P { x: Int }\n\
             let p = P { x: 1 };\n\
             println(p.m(1, 2));\n\
             impl P { fn m(self) -> Int { return self.x; } }\n",
        )
        .expect_err("the declared arity applies from above too")
        .to_string();
        assert!(message.contains("Method expects 0 arguments"), "{message}");
    }

    /// A store into a container is checked against what the container's type
    /// declares it holds — through every spelling of a store.
    ///
    /// The parser desugars each of them into a different hidden call
    /// (`list.set` for a literal index, `__lk_set_index` otherwise,
    /// `__lk_set_field` for a field), and none of the three checked the value.
    /// `l.set(0, "a")` on a `List<Int>` was refused while `l[0] = "a"` — the
    /// same operation, the other spelling — was accepted, and
    /// `let n: Int = l[0]` then type-checked and held a String.
    #[test]
    fn a_store_is_checked_against_what_the_container_declares() {
        let refused = [
            ("list element, literal index", "let l: List<Int> = [1];\nl[0] = \"a\";"),
            (
                "list element, variable index",
                "let l: List<Int> = [1];\nlet i = 0;\nl[i] = \"a\";",
            ),
            (
                "nested list element",
                "let l: List<List<Int>> = [[1]];\nl[0] = [\"a\"];",
            ),
            ("map value", "let m: Map<String, Int> = {\"k\": 1};\nm[\"k\"] = \"a\";"),
            ("map key", "let m: Map<String, Int> = {\"k\": 1};\nm[7] = 2;"),
            ("struct field", "struct S { x: Int }\nlet s = S { x: 1 };\ns.x = \"a\";"),
            (
                "the method spelling, which always was",
                "let l: List<Int> = [1];\nl.set(0, \"a\");",
            ),
            // A heterogeneous literal infers `Tuple`, whose positions have
            // different types — the carrier the first version of this check
            // did not cover.
            ("tuple position, literal index", "let l = [1, \"a\"];\nl[0] = 2.5;"),
            (
                "tuple, index not a literal",
                "let l = [1, \"a\"];\nlet i = 0;\nl[i] = 2.5;",
            ),
            (
                "annotated tuple position",
                "let l: Tuple<Int, String> = [1, \"a\"];\nl[0] = \"z\";",
            ),
        ];
        for (what, source) in refused {
            check_program(source).expect_err(what);
        }

        let accepted = [
            ("a store of the declared type", "let l: List<Int> = [1];\nl[0] = 2;"),
            ("compound assignment", "let l: List<Int> = [1];\nl[0] += 1;"),
            (
                "a map store of the declared types",
                "let m: Map<String, Int> = {\"k\": 1};\nm[\"j\"] = 2;",
            ),
            (
                "a struct field of its declared type",
                "struct S { x: Int }\nlet s = S { x: 1 };\ns.x = 2;",
            ),
            (
                "an untyped container still takes anything",
                "let l = [];\nl.push(1);\nl[0] = \"a\";",
            ),
            ("a tuple position of its own type", "let l = [1, \"a\"];\nl[1] = \"z\";"),
        ];
        for (what, source) in accepted {
            check_program(source).expect(what);
        }
    }

    /// A container cannot be widened at its element type, in any of the five
    /// positions that could do it.
    ///
    /// These containers are mutable and a widening is an *alias*, so the wide
    /// name can write an element the narrow name's type forbids:
    ///
    /// ```lk
    /// let a: List<Int> = [1, 2];
    /// let b: List<Any> = a;
    /// b.push("s");
    /// let c: Int = a[2];   // type-checked, and held "s"
    /// ```
    ///
    /// Closing only the parameter position would have left the other four.
    #[test]
    fn a_container_cannot_be_widened_at_its_element_type() {
        let widenings = [
            ("bare let", "let a: List<Int> = [1, 2];\nlet b: List = a;"),
            ("Any let", "let a: List<Int> = [1, 2];\nlet b: List<Any> = a;"),
            (
                "parameter",
                "fn take(xs: List) -> Int { return 0; }\nlet a: List<Int> = [1, 2];\nreturn take(a);",
            ),
            (
                "struct field",
                "struct Box { xs: List }\nlet a: List<Int> = [1, 2];\nlet b = Box { xs: a };",
            ),
            (
                "container element",
                "let a: List<Int> = [1, 2];\nlet holder: List<List> = [a];",
            ),
            ("return type", "fn widen(xs: List<Int>) -> List { return xs; }"),
            (
                "map value",
                "let m: Map<String, Int> = {\"k\": 1};\nlet w: Map<String, Any> = m;",
            ),
        ];
        for (what, source) in widenings {
            check_program(source).expect_err(what);
        }

        // A literal is a fresh container, so there is nothing to alias and the
        // annotation is simply what it is checked against.
        check_program("let a: List<Any> = [1, 2];").expect("a literal takes the declared element type");
        check_program("let a: List = [1, 2, 3];").expect("and so does a bare one");
        check_program("let m: Map<String, Any> = {\"k\": 1};").expect("map literals too");

        // `List<_>` — the read-only view — accepts every list, and writing
        // through it is refused because nothing is assignable to `_`.
        check_program("let a: List<Int> = [1, 2];\nlet n = a.zip([3, 4]);")
            .expect("a builtin that only reads its list argument takes any list");
        check_program("let a: List<_> = [1, 2];\na.push(3);").expect_err("a read-only view cannot be written");
    }

    #[test]
    fn a_list_satisfies_a_tuple_annotation_of_the_same_element_types() {
        check_program("let t: Tuple<Int, Int> = [1, 2];").expect("a two-Int list is a Tuple<Int, Int>");
        check_program("fn f() -> Tuple<Int, Int> { return [1, 2]; }").expect("and so is a returned one");
        check_program("let t: Tuple<Int, String> = [1, \"a\"];").expect("the heterogeneous case still works");
        check_program("let xs: List<Int> = [1, 2];\nlet t: Tuple<Int, Int> = xs;")
            .expect("through a variable too — the type is what is checked, not the literal");
        check_program("fn f(t: Tuple<Int, Int>) -> Int { return t[0]; }\nreturn f([1, 2]);")
            .expect("Tuple -> List still holds");

        check_program("let t: Tuple<Int, Int> = [\"a\", \"b\"];").expect_err("element types are still checked");
    }

    /// A constant condition does not delete the branch the checker has not
    /// seen.
    ///
    /// `fold_constants` runs in the parser, so anything it drops is dropped
    /// before name resolution and type checking ever run. `if false {
    /// undefined_fn() } else { 1 }`, `false && undefined_fn()` and `1 ??
    /// undefined_fn()` all passed `lk check` because the call was gone by the
    /// time anyone looked — the probe here uses a *type* error rather than an
    /// undefined name so it lands in this file's checker rather than in
    /// resolution. `-true` was worse than unchecked: it folded to `false` and
    /// printed it, while `-b` on a `Bool` variable is rejected.
    #[test]
    fn a_constant_condition_does_not_hide_the_branch_it_does_not_take() {
        check_program("let x = if false { -true } else { 1 };").expect_err("the untaken arm is still code");
        check_program("let x = if true { 1 } else { -true };").expect_err("and so is the other one");
        check_program("let x = false && -true;").expect_err("`&&` does not short-circuit the checker");
        check_program("let x = true || -true;").expect_err("nor does `||`");
        check_program("let x = 1 ?? -true;").expect_err("nor does `??`");
        check_program("let x = -true;").expect_err("negating a Bool is a type error, constant or not");

        // The folds that discard nothing but a literal still happen, and the
        // ordinary shapes still check.
        check_program("let x: Bool = false && true;").expect("both operands literal");
        check_program("let x: String = nil ?? \"a\";").expect("`nil ?? e` discards only the nil");
        check_program("let x: Int = -3;").expect("negating a literal");
        check_program("let x = if false { 1 } else { 2 };").expect("both arms literal");
    }

    /// Both arms of a constant `if` type, so the diagnostic names the union —
    /// the same thing a function body reports.
    ///
    /// It used to name whichever arm survived folding, which meant the
    /// top-level `let` and the identical `let` inside a function disagreed
    /// about what the expression's type even was.
    #[test]
    fn a_constant_conditional_reports_the_union_of_both_arms() {
        let message = check_program("let x: Int = if false { 9.5 } else { \"x\" };")
            .expect_err("Int accepts neither arm")
            .to_string();
        assert!(
            message.contains("Float | String"),
            "expected the union of both arms, got: {message}"
        );
    }

    /// A declared return type is a promise about every path.
    ///
    /// `fn g(c: Bool) -> Int { if c { return 1; } }` answered `nil` when `c` was
    /// false, and `lk check` said nothing: the failure surfaced at the caller as
    /// "Add expected numbers or strings, got Nil and Int", naming the operator
    /// rather than the function that promised an `Int`.
    ///
    /// Only annotations that exclude nil are held to it — `Nil`, `Any` and `T?`
    /// all admit the fall-through value, and an unannotated function's return
    /// type is inferred from what it returns, so there is no promise to break.
    #[test]
    fn a_declared_return_type_is_a_promise_about_every_path() {
        check_program("fn f(c: Bool) -> Int { if c { return 1; } }").expect_err("the false path falls through");
        check_program("fn f() -> Int { }").expect_err("so does an empty body");
        check_program("fn f() -> String { let x = 1; }").expect_err("and one that only computes");
        check_program("fn f(c: Bool) -> Int { if c { return 1; } else { let x = 2; } }")
            .expect_err("an else that does not return is still a path");

        // Every shape that does leave on every path.
        check_program("fn f(c: Bool) -> Int { if c { return 1; } else { return 2; } }").expect("both arms return");
        check_program("fn f(c: Bool) -> Int { if c { return 1; } return 2; }").expect("a trailing return");
        check_program("fn f() -> Int { while true { return 1; } }").expect("a loop with no way out");
        check_program("fn f(x: Int) -> Int { match x { 1 => { return 1; }, _ => { return 2; } } }")
            .expect("a match with a catch-all, every arm returning");
        check_program("fn f() -> Int { error(\"no\"); }").expect("a raise leaves too");
        check_program("fn f() -> Int { panic(\"x\"); }").expect("and so does a panic");
        check_program("fn f(c: Bool) -> Int { if c { return 1; } panic(\"x\"); }").expect("mixed");

        // And the annotations that admit the fall-through value.
        check_program("fn f() -> Nil { }").expect("Nil is what falling through answers");
        check_program("fn f() -> Int? { }").expect("an optional says it may answer nothing");
        check_program("fn f() -> Any { }").expect("Any admits it");
        check_program("fn f(c: Bool) { if c { return 1; } }").expect("no annotation, no promise");
    }
    /// `m + n` merges, and the checker had to be told.
    ///
    /// Both executors have implemented map merge all along — the VM's `Add`
    /// has a map arm and so does `lkrt_dyn_add` — and only the checker refused,
    /// so `a + b` ran when the types were erased to `Any` and was "the left
    /// operand must be numeric types" when they were not. `lk check` answers
    /// the executors' question; a rule it enforces that neither executor has is
    /// the same defect as a rule it misses.
    #[test]
    fn two_maps_merge_and_the_answer_widens_to_hold_both() {
        check_program("let a = {\"a\": 1};\nlet b = {\"b\": 2};\nlet c = a + b;\nprintln(c);\n")
            .expect("two maps merge");
        assert_eq!(
            infer("{\"a\": 1} + {\"b\": 2}"),
            Type::Map(Box::new(Type::String), Box::new(Type::Int)),
            "two `Map<String, Int>` merge into one"
        );
        assert_eq!(
            infer("{\"a\": 1} + {\"b\": \"s\"}"),
            Type::Map(Box::new(Type::String), Box::new(Type::Any)),
            "values that subsume neither widen to Any"
        );

        // A non-map on either side is still an error, and says what it expected
        // rather than borrowing the numeric operator's message.
        let err = check_program("let a = {\"a\": 1};\nlet c = a + 1;\nprintln(c);\n")
            .expect_err("a map plus a number is an error");
        assert!(
            format!("{err}").contains("map merge requires both operands to be maps"),
            "unexpected message: {err}"
        );
    }
}
