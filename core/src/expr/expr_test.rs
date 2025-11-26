#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use crate::{expr::Expr, val::Val, vm::VmContext};

    fn seed_env() -> VmContext {
        use std::collections::HashMap;
        let mut env = VmContext::new();

        // Common test bindings
        env.define("pub".to_string(), Val::Bool(true));

        let mut user = HashMap::new();
        user.insert("name".to_string(), Val::Str("lk".into()));
        user.insert("age".to_string(), Val::Int(18));
        env.define("user".to_string(), user.into());

        env.define(
            "list".to_string(),
            Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)].into()),
        );
        // Identifier with dash is allowed by lexer
        env.define("list-2".to_string(), Val::List(vec![Val::Int(2)].into()));

        // Helper index for index-based access tests
        env.define("index".to_string(), Val::Int(1));

        let mut nested_l2 = HashMap::new();
        nested_l2.insert("level2".to_string(), Val::Str("value".into()));
        let mut nested_l1: HashMap<String, Val> = HashMap::new();
        nested_l1.insert("level1".to_string(), nested_l2.into());
        env.define("nested".to_string(), nested_l1.into());

        env
    }

    #[test]
    fn simple() {
        expect("pub", true);
        expect("user.name + 'pt'", "lkpt");
        expect("user.age + list.0 == 19", true);
        expect("user.name + user.age", "lk18");
        expect("list + list-2", vec![1, 2, 3, 2]);
        expect("list - list-2", vec![1, 3]);
        expect("list.2 / 2", 1.5);
        panic("user.name + list");
    }

    #[test]
    fn complex_expressions() {
        // Nested arithmetic operations
        expect("(user.age + 2) * 3", 60);

        // Parenthesized expressions
        expect("user.age * (2 + 1)", 54);

        // Multiple operators with precedence
        expect("user.age + 2 * 3", 24);

        // Comparison operators
        expect("user.age > 17", true);
        expect("user.age < 19", true);
        expect("user.age >= 18", true);
        expect("user.age <= 18", true);
        expect("user.age == 18", true);
        expect("user.age != 19", true);
    }

    #[test]
    fn logical_operators() {
        // AND operator
        expect("pub && user.age > 17", true);
        expect("pub && user.age > 20", false);

        // OR operator
        expect("user.age > 20 || pub", true);
        expect("user.age > 20 || user.name == 'john'", false);

        // Complex logical expressions
        expect("pub && (user.age > 17 || user.name == 'john')", true);
        expect("pub || (user.age < 17 && user.name == 'john')", true);

        // Short-circuit evaluation (RHS not evaluated)
        expect("false && nonexistent.field", false);
        expect("true || nonexistent.field", true);
    }

    #[test]
    fn ternary_operator() {
        // Basic boolean conditions
        expect("true ? 1 : 2", 1);
        expect("false ? 1 : 2", 2);

        // With bound variables
        expect("pub ? user.name : 'guest'", "lk");

        // Short-circuit: only selected branch should evaluate
        expect("false ? (nonexistent.field) : 42", 42);

        // Precedence with arithmetic on else branch
        expect("true ? 1 : 2 + 3", 1);
        expect("false ? 1 : 2 + 3", 5);

        // Nested ternary
        expect("true ? (false ? 1 : 2) : 3", 2);

        // Nullish inside else branch
        expect("false ? 1 : (nil ?? 5)", 5);

        // Ternary as map key requires parentheses to avoid ambiguity with ':'
        use std::collections::HashMap;
        let mut expected = HashMap::new();
        expected.insert("x".to_string(), Val::Int(1));
        expect("{('a' == 'a' ? 'x' : 'y'): 1}", expected);
    }

    #[test]
    fn nullish_coalescing_operations() {
        // Basic nullish coalescing with nil values (missing property)
        expect("user.nonexistent ?? 'default'", "default");
        expect("user.name ?? 'default'", "lk");
        expect("nil ?? 'fallback'", "fallback");
        expect("'actual' ?? 'fallback'", "actual");

        // Numeric nullish coalescing
        expect("user.nonexistent ?? 18", 18);
        expect("user.age ?? 100", 18);

        // Boolean nullish coalescing
        expect("user.nonexistent ?? true", true);
        expect("pub ?? false", true);

        // Complex expressions with nullish coalescing
        expect("user.nonexistent ?? user.name ?? 'unknown'", "lk");
        expect("user.name ?? user.age ?? 'fallback'", "lk");

        // Nested nullish coalescing with other operators
        expect("(user.nonexistent ?? 5) + 10", 15);
        expect("(user.name ?? 'guest') == 'lk'", true);
        expect("user.name ?? ('guest' == 'lk')", "lk");

        // Constant folding
        expect("'hello' ?? 'world'", "hello");
        expect("nil ?? 'constant'", "constant");
    }

    #[test]
    fn unary_operations() {
        // Logical NOT
        expect("!pub", false);
        expect("!false", true);

        // Double negation
        expect("!!pub", true);

        // NOT with expressions
        expect("!(user.age > 20)", true);
    }

    #[test]
    fn map_and_list_access() {
        // Nested map access
        expect("nested.level1.level2", "value");

        // List access with variable index
        expect("list[(index)]", 2);

        // Access with expressions
        // `index-1` is an Id, but `index - 1` is a BinOp
        expect("list[(index - 1)]", 1);

        // Access with complex expressions
        expect("list-2[(2 - 2)] + user.name", "2lk");
    }

    #[test]
    fn list_literals() {
        // Empty list
        expect("[]", Vec::<Val>::new());

        // Simple list
        expect("[1, 2, 3]", vec![1, 2, 3]);

        // Mixed types
        expect(
            r#"[1, "hello", true]"#,
            vec![Val::Int(1), Val::Str("hello".into()), Val::Bool(true)],
        );

        // Nested lists
        expect("[[1, 2], [3, 4]]", vec![vec![1, 2], vec![3, 4]]);

        // List with expressions
        expect("[1 + 2, 3 * 4]", vec![3, 12]);

        // List with variable access
        expect("[user.age, list.0]", vec![18, 1]);
    }

    #[test]
    fn map_literals() {
        use std::collections::HashMap;

        // Empty map
        expect("{}", HashMap::<String, Val>::new());

        // Simple map
        let mut expected = HashMap::new();
        expected.insert("name".to_string(), Val::Str("Alice".into()));
        expected.insert("age".to_string(), Val::Int(30));
        expect(r#"{"name": "Alice", "age": 30}"#, expected);

        // Map with expressions
        let mut expected = HashMap::new();
        expected.insert("sum".to_string(), Val::Int(5));
        expected.insert("product".to_string(), Val::Int(6));
        expect(r#"{"sum": 2 + 3, "product": 2 * 3}"#, expected);

        // Map with member access
        let mut expected = HashMap::new();
        expected.insert("user_name".to_string(), Val::Str("lk".into()));
        expected.insert("user_age".to_string(), Val::Int(18));
        expect(r#"{"user_name": user.name, "user_age": user.age}"#, expected);

        // Map with different key types
        let mut expected = HashMap::new();
        expected.insert("42".to_string(), Val::Str("number".into()));
        expected.insert("true".to_string(), Val::Str("bool".into()));
        expected.insert("key".to_string(), Val::Str("string".into()));
        expect(r#"{42: "number", true: "bool", "key": "string"}"#, expected);
    }

    #[test]
    fn nested_structures() {
        use std::collections::HashMap;

        // List of maps
        let mut map1 = HashMap::new();
        map1.insert("name".to_string(), Val::Str("Alice".into()));
        map1.insert("age".to_string(), Val::Int(30));

        let mut map2 = HashMap::new();
        map2.insert("name".to_string(), Val::Str("Bob".into()));
        map2.insert("age".to_string(), Val::Int(25));

        expect(
            r#"[{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}]"#,
            vec![Val::from(map1), Val::from(map2)],
        );

        // Map with lists
        let mut expected = HashMap::new();
        expected.insert(
            "numbers".to_string(),
            Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)].into()),
        );
        expected.insert("active".to_string(), Val::Bool(true));
        expect(r#"{"numbers": [1, 2, 3], "active": true}"#, expected);
    }

    #[test]
    fn literal_access() {
        // Access elements from list literals
        expect("[1, 2, 3].1", 2);
        expect(r#"["hello", "world"].0"#, "hello");

        // Access fields from map literals
        expect(r#"{"name": "Alice", "age": 30}.name"#, "Alice");
        expect(r#"{"name": "Alice", "age": 30}.age"#, 30);

        // Nested access
        expect(r#"[{"name": "Alice"}, {"name": "Bob"}].0.name"#, "Alice");
        expect(r#"{"users": [1, 2, 3]}.users.1"#, 2);
    }

    #[test]
    fn bracket_index_access() {
        // List indexing with brackets
        expect("[1, 2, 3][1]", 2);
        expect(r#"["hello", "world"][0]"#, "hello");

        // Map indexing with string key
        expect(r#"{"name": "Alice", "age": 30}["name"]"#, "Alice");

        // Mixed bracket and dot access
        expect(r#"{ "a": [10, 20, 30] }["a"][2]"#, 30);
    }

    #[test]
    fn trailing_commas() {
        // List with trailing comma
        expect("[1, 2, 3,]", vec![1, 2, 3]);

        // Map with trailing comma
        use std::collections::HashMap;
        let mut expected = HashMap::new();
        expected.insert("a".to_string(), Val::Int(1));
        expected.insert("b".to_string(), Val::Int(2));
        expect(r#"{"a": 1, "b": 2,}"#, expected);
    }

    #[test]
    fn error_cases() {
        // Invalid map key types
        panic(r#"{[1, 2]: "invalid"}"#);
        panic(r#"{{}: "invalid"}"#);
    }

    #[test]
    fn test_nil_handling() {
        expect("nil == nil", true);
        expect("user.nonexistent == nil", true);
        expect("nil", None::<Val>);
    }

    // 缺失 Optional Chanining 测试

    #[test]
    fn optional_chaining_access_and_index() {
        let mut ctx = seed_env();

        // Existing path succeeds with or without optional access
        let expr = Expr::try_from("nested?.level1?.level2").unwrap();
        let out = expr.eval_with_ctx(&mut ctx).unwrap();
        assert_eq!(out, Val::Str("value".into()));

        // Missing intermediate field yields nil rather than error
        let expr = Expr::try_from("nested?.missing?.level2").unwrap();
        let out = expr.eval_with_ctx(&mut ctx).unwrap();
        assert_eq!(out, Val::Nil);

        // Optional access at the end also yields nil on missing leaf
        let expr = Expr::try_from("user?.missing").unwrap();
        let out = expr.eval_with_ctx(&mut ctx).unwrap();
        assert_eq!(out, Val::Nil);

        // Optional bracket index on list out-of-bounds returns nil
        let expr = Expr::try_from("list?[10]").unwrap();
        let out = expr.eval_with_ctx(&mut ctx).unwrap();
        assert_eq!(out, Val::Nil);

        // Optional bracket index on existing element returns the value
        let expr = Expr::try_from("list?[1]").unwrap();
        let out = expr.eval_with_ctx(&mut ctx).unwrap();
        assert_eq!(out, Val::Int(2));
    }

    fn expect<V: Into<Val> + Clone>(rule: &str, val: V) {
        let expr = Expr::try_from(rule).unwrap();
        let mut ctx = seed_env();
        let res = expr.eval_with_ctx(&mut ctx);
        assert_eq!(res.unwrap(), val.into());
    }

    fn panic(rule: &str) {
        match Expr::try_from(rule) {
            Ok(expr) => {
                let mut ctx = seed_env();
                let res = expr.eval_with_ctx(&mut ctx);
                assert!(res.is_err());
                let err = res.unwrap_err();
                println!("{}", err);
            }
            Err(e) => {
                // Parsing itself failed; this is also an expected failure path for this helper
                println!("parse error: {}", e);
            }
        }
    }

    #[test]
    fn range_expressions() {
        // Exclusive range
        expect("1..5", vec![1, 2, 3, 4]);

        // Inclusive range
        expect("1..=5", vec![1, 2, 3, 4, 5]);

        // Single element inclusive range
        expect("1..=1", vec![1]);

        // Empty exclusive range
        expect("5..5", Vec::<Val>::new());

        // Negative ranges
        expect("-3..=3", vec![-3, -2, -1, 0, 1, 2, 3]);
    }

    // 缺失 Closure 测试

    #[test]
    fn template_strings() {
        // Basic template string with no interpolation
        expect("\"Hello, World!\"", "Hello, World!");

        // Template string with simple variable interpolation using ${}
        expect("\"Hello, ${user.name}!\"", "Hello, lk!");

        // Template string with multiple interpolations
        expect(
            "\"User ${user.name} is ${user.age} years old\"",
            "User lk is 18 years old",
        );

        // Template string with expressions
        expect("\"Next year: ${user.age + 1}\"", "Next year: 19");

        // Template string with list access
        expect("\"First item: ${list.0}\"", "First item: 1");

        // Template string with boolean expressions
        expect("\"Is adult: ${user.age >= 18}\"", "Is adult: true");

        // Template string with arithmetic operations
        expect("\"Sum: ${list.0 + list.1}\"", "Sum: 3");

        // Template string with nested access
        expect("\"Nested: ${nested.level1.level2}\"", "Nested: value");

        // Template string with special characters (escaped)
        expect(
            "\"Escaped: \\\"quote\\\" and \\$dollar\"",
            "Escaped: \"quote\" and $dollar",
        );

        // Template string with nil value
        expect("\"Nil test: ${user.nonexistent}\"", "Nil test: nil");

        // Template string with complex expressions
        expect("\"Calculation: ${(user.age * 2) + 5}\"", "Calculation: 41");

        // Empty template string
        expect("\"\"", "");

        // Template string with only interpolation
        expect("\"${user.name}\"", "lk");
    }

    #[test]
    fn template_string_constant_folding() {
        // Test that template strings with constant expressions are folded
        let expr = Expr::try_from("\"Hello \\\"World\\\"!\"").unwrap();
        // Should fold to a single string constant during parsing
        if let Expr::Val(Val::Str(s)) = expr {
            assert_eq!(s.as_ref(), "Hello \"World\"!");
        } else {
            panic!("Template string with constants should be folded to Val");
        }

        // Test that template strings with variables are not folded
        let expr = Expr::try_from("\"Hello ${user.name}!\"").unwrap();
        assert!(matches!(expr, Expr::TemplateString(_)));
    }

    #[test]
    fn template_string_error_cases() {
        // Test unclosed template expression
        panic("\"Hello ${user.name\"");

        // Test invalid expression in template string
        panic("\"Hello ${user. + 1}!\"");
    }

    #[test]
    fn template_string_identifier_collection() {
        // Test that template strings correctly collect identifier roots
        let expr = Expr::try_from("\"Hello ${user.name}, your items are ${items.0} and ${items.1}\"").unwrap();
        let ctx_names = expr.requested_ctx();
        assert_eq!(ctx_names, HashSet::from(["user".to_string(), "items".to_string()]));
    }
}
