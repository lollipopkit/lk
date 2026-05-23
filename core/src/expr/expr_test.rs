#[cfg(test)]
mod test {
    use std::collections::HashSet;

    use crate::{
        expr::Expr,
        val::Val,
        vm::{VmContext, execute_source32},
    };

    fn seed_env() -> VmContext {
        let mut env = VmContext::new();

        env.set_val_binding("pub".to_string(), Val::Bool(true));
        env
    }

    #[test]
    fn simple() {
        expect("pub", true);
        expect32_env("user.name + 'pt'", "lkpt");
        expect32_env("user.age + list.0 == 19", "true");
        expect32_env("user.name + user.age", "lk18");
        expect32("[1, 2, 3] + [2]", "[1, 2, 3, 2]");
        expect32("[1, 2, 3] - [2]", "[1, 3]");
        expect32_env("list.2 / 2.0", "1.5");
        panic32_env("user.name / list");
    }

    #[test]
    fn complex_expressions() {
        // Nested arithmetic operations
        expect32_env("(user.age + 2) * 3", "60");

        // Parenthesized expressions
        expect32_env("user.age * (2 + 1)", "54");

        // Multiple operators with precedence
        expect32_env("user.age + 2 * 3", "24");

        // Comparison operators
        expect32_env("user.age > 17", "true");
        expect32_env("user.age < 19", "true");
        expect32_env("user.age >= 18", "true");
        expect32_env("user.age <= 18", "true");
        expect32_env("user.age == 18", "true");
        expect32_env("user.age != 19", "true");
    }

    #[test]
    fn logical_operators() {
        // AND operator
        expect32_env("pub && user.age > 17", "true");
        expect32_env("pub && user.age > 20", "false");

        // OR operator
        expect32_env("user.age > 20 || pub", "true");
        expect32_env("user.age > 20 || user.name == 'john'", "false");

        // Complex logical expressions
        expect32_env("pub && (user.age > 17 || user.name == 'john')", "true");
        expect32_env("pub || (user.age < 17 && user.name == 'john')", "true");

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
        expect32_env("pub ? user.name : 'guest'", "lk");

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
        expect32("{('a' == 'a' ? 'x' : 'y'): 1}.x", "1");
    }

    #[test]
    fn nullish_coalescing_operations() {
        // Basic nullish coalescing with nil values (missing property)
        expect32_env("user.nonexistent ?? 'default'", "default");
        expect32_env("user.name ?? 'default'", "lk");
        expect("nil ?? 'fallback'", "fallback");
        expect("'actual' ?? 'fallback'", "actual");

        // Numeric nullish coalescing
        expect32_env("user.nonexistent ?? 18", "18");
        expect32_env("user.age ?? 100", "18");

        // Boolean nullish coalescing
        expect32_env("user.nonexistent ?? true", "true");
        expect32_env("pub ?? false", "true");

        // Complex expressions with nullish coalescing
        expect32_env("user.nonexistent ?? user.name ?? 'unknown'", "lk");
        expect32_env("user.name ?? user.age ?? 'fallback'", "lk");

        // Nested nullish coalescing with other operators
        expect32_env("(user.nonexistent ?? 5) + 10", "15");
        expect32_env("(user.name ?? 'guest') == 'lk'", "true");
        expect32_env("user.name ?? ('guest' == 'lk')", "lk");

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
        expect32_env("!(user.age > 20)", "true");
    }

    #[test]
    fn map_and_list_access() {
        // Nested map access
        expect32_env("nested.level1.level2", "value");

        // List access with variable index
        expect32_env("list[(index)]", "2");

        // Access with expressions
        // `index-1` is an Id, but `index - 1` is a BinOp
        expect32_env("list[(index - 1)]", "1");

        // Access with complex expressions
        expect32_env("[2][(2 - 2)] + user.name", "2lk");
    }

    #[test]
    fn list_literals() {
        // Empty list
        expect32("[]", "[]");

        // Simple list
        expect32("[1, 2, 3]", "[1, 2, 3]");

        // Mixed types
        expect32(r#"[1, "hello", true]"#, "[1, hello, true]");

        // Nested lists
        expect32("[[1, 2], [3, 4]]", "[[1, 2], [3, 4]]");

        // List with expressions
        expect32("[1 + 2, 3 * 4]", "[3, 12]");

        // List with variable access
        expect32_source(
            r#"
            let user = {"age": 18};
            let list = [1, 2, 3];
            return [user.age, list.0];
            "#,
            "[18, 1]",
        );
    }

    #[test]
    fn map_literals() {
        // Empty map
        expect32("{}", "{}");

        // Simple map
        expect32(r#"{"name": "Alice", "age": 30}.name"#, "Alice");
        expect32(r#"{"name": "Alice", "age": 30}.age"#, "30");

        // Map with expressions
        expect32(r#"{"sum": 2 + 3, "product": 2 * 3}.sum"#, "5");
        expect32(r#"{"sum": 2 + 3, "product": 2 * 3}.product"#, "6");

        // Map with member access
        expect32_source(
            r#"
            let user = {"name": "lk", "age": 18};
            return {"user_name": user.name, "user_age": user.age}.user_name;
            "#,
            "lk",
        );
        expect32_source(
            r#"
            let user = {"name": "lk", "age": 18};
            return {"user_name": user.name, "user_age": user.age}.user_age;
            "#,
            "18",
        );

        // Map with different key types
        expect32(r#"{42: "number", true: "bool", "key": "string"}[42]"#, "number");
        expect32(r#"{42: "number", true: "bool", "key": "string"}[true]"#, "bool");
        expect32(r#"{42: "number", true: "bool", "key": "string"}.key"#, "string");
    }

    #[test]
    fn nested_structures() {
        // List of maps
        expect32(
            r#"[{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}].0.name"#,
            "Alice",
        );
        expect32(
            r#"[{"name": "Alice", "age": 30}, {"name": "Bob", "age": 25}].1.age"#,
            "25",
        );

        // Map with lists
        expect32(r#"{"numbers": [1, 2, 3], "active": true}.numbers.2"#, "3");
        expect32(r#"{"numbers": [1, 2, 3], "active": true}.active"#, "true");
    }

    #[test]
    fn literal_access() {
        // Access elements from list literals
        expect32("[1, 2, 3].1", "2");
        expect32(r#"["hello", "world"].0"#, "hello");

        // Access fields from map literals
        expect32(r#"{"name": "Alice", "age": 30}.name"#, "Alice");
        expect32(r#"{"name": "Alice", "age": 30}.age"#, "30");

        // Nested access
        expect32(r#"[{"name": "Alice"}, {"name": "Bob"}].0.name"#, "Alice");
        expect32(r#"{"users": [1, 2, 3]}.users.1"#, "2");
    }

    #[test]
    fn bracket_index_access() {
        // List indexing with brackets
        expect32("[1, 2, 3][1]", "2");
        expect32(r#"["hello", "world"][0]"#, "hello");

        // Map indexing with string key
        expect32(r#"{"name": "Alice", "age": 30}["name"]"#, "Alice");

        // Mixed bracket and dot access
        expect32(r#"{ "a": [10, 20, 30] }["a"][2]"#, "30");
    }

    #[test]
    fn trailing_commas() {
        // List with trailing comma
        expect32("[1, 2, 3,]", "[1, 2, 3]");

        // Map with trailing comma
        expect32(r#"{"a": 1, "b": 2,}.a"#, "1");
        expect32(r#"{"a": 1, "b": 2,}.b"#, "2");
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
        expect32_env("user.nonexistent == nil", "true");
        expect("nil", None::<Val>);
    }

    // 缺失 Optional Chanining 测试

    #[test]
    fn optional_chaining_access_and_index() {
        // Existing path succeeds with or without optional access
        expect32_env("nested?.level1?.level2", "value");

        // Missing intermediate field yields nil rather than error
        expect32_env("nested?.missing?.level2 == nil", "true");

        // Optional access at the end also yields nil on missing leaf
        expect32_env("user?.missing == nil", "true");

        // Optional bracket index on list out-of-bounds returns nil
        expect32_env("list?[10] == nil", "true");

        // Optional bracket index on existing element returns the value
        expect32_env("list?[1]", "2");
    }

    fn expect<V: crate::val::TestIntoVal>(rule: &str, val: V) {
        let expr = Expr::try_from(rule).unwrap();
        let mut ctx = seed_env();
        let res = expr.eval_with_ctx(&mut ctx);
        assert_eq!(res.unwrap(), Val::test_from(val));
    }

    fn expect32(rule: &str, expected_display: &str) {
        expect32_source(&format!("return {rule};"), expected_display);
    }

    fn expect32_source(source: &str, expected_display: &str) {
        let result = execute_source32(source).expect("execute source");
        assert_eq!(result.display_first_return(), expected_display);
    }

    fn expect32_env(rule: &str, expected_display: &str) {
        expect32_source(
            &format!(
                r#"
                let pub = true;
                let user = {{"name": "lk", "age": 18}};
                let list = [1, 2, 3];
                let index = 1;
                let nested = {{"level1": {{"level2": "value"}}}};
                return {rule};
                "#
            ),
            expected_display,
        );
    }

    fn panic32_env(rule: &str) {
        let source = format!(
            r#"
            let pub = true;
            let user = {{"name": "lk", "age": 18}};
            let list = [1, 2, 3];
            let index = 1;
            let nested = {{"level1": {{"level2": "value"}}}};
            return {rule};
            "#
        );
        assert!(execute_source32(&source).is_err());
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
        expect32("1..5", "[1, 2, 3, 4]");

        // Inclusive range
        expect32("1..=5", "[1, 2, 3, 4, 5]");

        // Single element inclusive range
        expect32("1..=1", "[1]");

        // Empty exclusive range
        expect32("5..5", "[]");

        // Negative ranges
        expect32("-3..=3", "[-3, -2, -1, 0, 1, 2, 3]");
    }

    // 缺失 Closure 测试

    #[test]
    fn template_strings() {
        // Basic template string with no interpolation
        expect("\"Hello, World!\"", "Hello, World!");

        // Template string with simple variable interpolation using ${}
        expect32_env("\"Hello, ${user.name}!\"", "Hello, lk!");

        // Template string with multiple interpolations
        expect32_env(
            "\"User ${user.name} is ${user.age} years old\"",
            "User lk is 18 years old",
        );

        // Template string with expressions
        expect32_env("\"Next year: ${user.age + 1}\"", "Next year: 19");

        // Template string with list access
        expect32_env("\"First item: ${list.0}\"", "First item: 1");

        // Template string with boolean expressions
        expect32_env("\"Is adult: ${user.age >= 18}\"", "Is adult: true");

        // Template string with arithmetic operations
        expect32_env("\"Sum: ${list.0 + list.1}\"", "Sum: 3");

        // Template string with nested access
        expect32_env("\"Nested: ${nested.level1.level2}\"", "Nested: value");

        // Template string with special characters (escaped)
        expect(
            "\"Escaped: \\\"quote\\\" and \\$dollar\"",
            "Escaped: \"quote\" and $dollar",
        );

        // Template string with nil value
        expect32_env("\"Nil test: ${user.nonexistent}\"", "Nil test: nil");

        // Template string with complex expressions
        expect32_env("\"Calculation: ${(user.age * 2) + 5}\"", "Calculation: 41");

        // Empty template string
        expect("\"\"", "");

        // Template string with only interpolation
        expect32_env("\"${user.name}\"", "lk");
    }

    #[test]
    fn template_string_constant_folding() {
        // Test that template strings with constant expressions are folded
        let expr = Expr::try_from("\"Hello \\\"World\\\"!\"").unwrap();
        // Should fold to a single string constant during parsing
        if let Expr::Val(v) = expr {
            assert_eq!(v.as_str(), Some("Hello \"World\"!"));
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
