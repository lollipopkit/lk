#[cfg(test)]
mod tests {
    use super::super::resolve::slots::*; // when compiled via lib path
    use crate::stmt::{Program, stmt_parser::StmtParser};
    use crate::token::Tokenizer;

    fn parse_program(input: &str) -> Program {
        let tokens = Tokenizer::tokenize(input).expect("tokenize");
        let mut sp = StmtParser::new(&tokens);
        sp.parse_program().expect("parse program")
    }

    #[test]
    fn test_basic_slots_and_function() {
        let program = parse_program(
            r#"
            let a = 1;
            let b = 2;
            { let a = 3; }
            fn inc(x) { let y = x; return y; }
            "#,
        );
        let mut resolver = SlotResolver::new();
        let res = resolver.resolve_program_slots(&program);

        // Root decls should allocate sequential indices in order of discovery
        let decls = &res.root.decls;
        assert_eq!(decls.len(), 4, "root decl count");
        assert_eq!(decls[0].name, "a");
        assert_eq!(decls[0].index, 0);
        assert_eq!(decls[1].name, "b");
        assert_eq!(decls[1].index, 1);
        assert_eq!(decls[2].name, "a", "shadowed 'a' gets a new slot");
        assert_eq!(decls[2].index, 2);
        assert!(!decls[2].is_param);
        assert_eq!(decls[3].name, "inc");
        assert_eq!(decls[3].index, 3);

        // One child function layout for inc
        assert_eq!(res.root.children.len(), 1, "inc child function");
        let inc = &res.root.children[0];
        // inc params + local y
        assert!(inc.total_locals >= 2);
        // First decl should be param x at index 0
        assert_eq!(inc.decls[0].name, "x");
        assert!(inc.decls[0].is_param);
        assert_eq!(inc.decls[0].index, 0);
        // y is a local
        assert!(inc.decls.iter().any(|d| d.name == "y" && !d.is_param && d.index == 1));

        // Uses inside inc: x and y referenced
        let mut saw_x = false;
        let mut saw_y = false;
        for u in &inc.uses {
            if u.name == "x" && u.slot.depth == 0 && u.slot.index == 0 {
                saw_x = true;
            }
            if u.name == "y" && u.slot.depth == 0 && u.slot.index == 1 {
                saw_y = true;
            }
        }
        assert!(saw_x, "x used in inc body");
        assert!(saw_y, "y used in inc body");
    }

    #[test]
    fn test_closure_child_layout() {
        let program = parse_program(
            r#"
            let f = |x| x;
            "#,
        );
        let mut resolver = SlotResolver::new();
        let res = resolver.resolve_program_slots(&program);

        // f should be declared at root
        assert!(res.root.decls.iter().any(|d| d.name == "f" && d.index == 0));
        // Should have exactly one anonymous child for the closure
        assert_eq!(res.root.children.len(), 1, "one closure child");
        let closure = &res.root.children[0];
        // Param x at index 0, and a use of x
        assert_eq!(closure.decls[0].name, "x");
        assert!(closure.decls[0].is_param);
        assert_eq!(closure.decls[0].index, 0);
        assert!(closure
            .uses
            .iter()
            .any(|u| u.name == "x" && u.slot.depth == 0 && u.slot.index == 0));
    }
}
