#[cfg(test)]
mod tests {
    use crate::vm::execute_source32;

    fn expect_expr(expr: &str, expected_display: &str) {
        let result = execute_source32(&format!("return {expr};")).expect("execute source");
        assert_eq!(result.display_first_return(), expected_display);
    }

    // Tests with literal expressions
    #[test]
    fn literal_list_operations() {
        expect_expr("([1, 2, 3]) + ([4, 5])", "[1, 2, 3, 4, 5]");
        expect_expr("([1, 2, 3]) - ([2])", "[1, 3]");
        expect_expr("([1, 2, 3]) - 2", "[1, 3]");
    }

    #[test]
    fn literal_map_operations() {
        let expr = r#"({"a": 1, "b": 2}) + ({"c": 3, "a": 4})"#;
        expect_expr(&format!("({expr}).a"), "4");
        expect_expr(&format!("({expr}).b"), "2");
        expect_expr(&format!("({expr}).c"), "3");

        expect_expr(r#"(({"a": 1, "b": 2}) - ({"a": 9})).a == nil"#, "true");
        expect_expr(r#"(({"a": 1, "b": 2}) - ({"a": 9})).b"#, "2");
        expect_expr(r#"(({"a": 1, "b": 2}) - "a").a == nil"#, "true");
        expect_expr(r#"(({"a": 1, "b": 2}) - "a").b"#, "2");
    }

    #[test]
    fn nested_literal_comparisons() {
        // Compare nested lists
        expect_expr("[[1, 2], [3, 4]] == [[1, 2], [3, 4]]", "true");
        expect_expr("[[1, 2], [3, 4]] == [[1, 2], [3, 5]]", "false");

        // Compare nested maps
        expect_expr(r#"{"user": {"name": "Alice"}} == {"user": {"name": "Alice"}}"#, "true");
        expect_expr(r#"{"user": {"name": "Alice"}} == {"user": {"name": "Bob"}}"#, "false");
    }

    #[test]
    fn mixed_type_comparisons() {
        // List vs non-list
        expect_expr("[1, 2, 3] == 123", "false");

        // Map vs non-map
        expect_expr(r#"{"a": 1} == 1"#, "false");

        // Empty structures
        expect_expr("[] == {}", "false");
    }
}
