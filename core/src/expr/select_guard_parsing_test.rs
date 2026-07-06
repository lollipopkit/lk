//! `select` is parse-time sugar (like `try`/`catch` → `pcall`): the parser
//! desugars it onto the `select$block` runtime builtin plus ordinary
//! let/list/conditional AST — there is no `Expr::Select` node. These tests
//! pin the desugared shape; behavioral coverage lives in the stdlib crate
//! (which owns the `select$block` native) and `examples/syntax`.

#[cfg(test)]
mod tests {
    use crate::{
        ast::Parser,
        expr::Expr,
        stmt::Stmt,
        token::Tokenizer,
        val::LiteralVal,
    };

    fn parse(code: &str) -> Expr {
        let tokens = Tokenizer::tokenize(code).unwrap();
        Parser::new(&tokens).parse().unwrap()
    }

    /// Walks the desugared block: returns (select$block call args, trailing
    /// dispatch expression).
    fn desugared_parts(expr: &Expr) -> (&[Box<Expr>], &Expr) {
        let Expr::Block(statements) = expr else {
            panic!("select must desugar to a block expression, got {expr:?}");
        };
        let mut call_args: Option<&[Box<Expr>]> = None;
        for stmt in statements {
            if let Stmt::Let { value, .. } = stmt.as_ref()
                && let Expr::Call(name, args) = value.as_ref()
                && name == "select$block"
            {
                call_args = Some(args);
            }
        }
        let Some(Stmt::Expr(dispatch)) = statements.last().map(|s| s.as_ref()) else {
            panic!("desugared select must end in a dispatch expression");
        };
        (
            call_args.expect("desugared select must call select$block"),
            dispatch.as_ref(),
        )
    }

    #[test]
    fn select_with_guard_desugars_to_select_block_call() {
        let expr = parse(
            r#"
            select {
                case recv(ch) if true => 1;
                default => 0;
            }
            "#,
        );
        let (args, dispatch) = desugared_parts(&expr);
        assert_eq!(args.len(), 5, "types/channels/values/guards/has_default");
        // One recv arm: types == [0]
        let Expr::List(types) = args[0].as_ref() else {
            panic!("first arg must be the types list");
        };
        assert_eq!(types.len(), 1);
        assert_eq!(*types[0].as_ref(), Expr::Literal(LiteralVal::Int(0)));
        // default present: has_default == true
        assert_eq!(*args[4].as_ref(), Expr::Literal(LiteralVal::Bool(true)));
        // Dispatch is the is_default conditional
        assert!(matches!(dispatch, Expr::Conditional(..)));
    }

    #[test]
    fn send_arm_evaluates_value_eagerly_and_no_default_passes_false() {
        let expr = parse("select { case send(ch, x + 1) => 2; }");
        let (args, _) = desugared_parts(&expr);
        let Expr::List(types) = args[0].as_ref() else {
            panic!("first arg must be the types list");
        };
        assert_eq!(*types[0].as_ref(), Expr::Literal(LiteralVal::Int(1)), "send kind is 1");
        // The send value is hoisted into a synthesized local, not inlined.
        let Expr::List(values) = args[2].as_ref() else {
            panic!("third arg must be the values list");
        };
        assert!(
            matches!(values[0].as_ref(), Expr::Var(name) if name.starts_with("__select")),
            "send value must be hoisted: {:?}",
            values[0]
        );
        assert_eq!(*args[4].as_ref(), Expr::Literal(LiteralVal::Bool(false)));
    }

    #[test]
    fn recv_binding_becomes_a_let_in_the_arm_block() {
        let expr = parse("select { case v <- recv(ch) => v; }");
        let (_, dispatch) = desugared_parts(&expr);
        // is_default conditional → arm-index conditional → arm block with the binding let
        let Expr::Conditional(_, _, arms) = dispatch else {
            panic!("dispatch must be a conditional");
        };
        let Expr::Conditional(_, arm_body, _) = arms.as_ref() else {
            panic!("arm dispatch must be a conditional");
        };
        let Expr::Block(arm_statements) = arm_body.as_ref() else {
            panic!("a recv arm with a binding must be a block");
        };
        assert!(
            matches!(
                arm_statements.first().map(|s| s.as_ref()),
                Some(Stmt::Let { pattern, .. })
                    if format!("{pattern:?}").contains("\"v\"")
            ),
            "binding `v` must be introduced by a let: {arm_statements:?}"
        );
    }

    #[test]
    fn nested_selects_use_distinct_synthesized_names() {
        let expr = parse(
            r#"
            select {
                case recv(a) => select { case recv(b) => 1; default => 2; };
                default => 3;
            }
            "#,
        );
        let rendered = format!("{expr:?}");
        // Two desugar instances → two distinct counters in synthesized names.
        assert!(rendered.contains("__select1_r"), "outer or inner select id 1: {rendered}");
        assert!(rendered.contains("__select2_r"), "outer or inner select id 2: {rendered}");
    }
}
