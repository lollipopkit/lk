#[cfg(test)]
mod tests {
    use crate::{expr::Expr, val::Val};

    // Tests with literal expressions
    #[test]
    fn literal_list_operations() {
        let expr: Expr = "([1, 2, 3]) + ([4, 5])".try_into().unwrap();
        let result = expr.eval().unwrap();
        let expected: Val = vec![1, 2, 3, 4, 5].into();
        assert_eq!(result, expected);
    }

    #[test]
    fn literal_map_operations() {
        let expr: Expr = r#"({"a": 1, "b": 2}) + ({"c": 3, "a": 4})"#.try_into().unwrap();
        let result = expr.eval().unwrap();

        // The result should be a map with "a": 4, "b": 2, "c": 3
        if let Val::Map(map) = result {
            assert_eq!(map.get("a"), Some(&Val::Int(4)));
            assert_eq!(map.get("b"), Some(&Val::Int(2)));
            assert_eq!(map.get("c"), Some(&Val::Int(3)));
        } else {
            panic!("Expected map result");
        }
    }

    #[test]
    fn nested_literal_comparisons() {
        // Compare nested lists
        let expr: Expr = "[[1, 2], [3, 4]] == [[1, 2], [3, 4]]".try_into().unwrap();
        let result = expr.eval().unwrap();
        assert_eq!(result, Val::Bool(true));

        let expr: Expr = "[[1, 2], [3, 4]] == [[1, 2], [3, 5]]".try_into().unwrap();
        let result = expr.eval().unwrap();
        assert_eq!(result, Val::Bool(false));

        // Compare nested maps
        let expr: Expr = r#"{"user": {"name": "Alice"}} == {"user": {"name": "Alice"}}"#.try_into().unwrap();
        let result = expr.eval().unwrap();
        assert_eq!(result, Val::Bool(true));

        let expr: Expr = r#"{"user": {"name": "Alice"}} == {"user": {"name": "Bob"}}"#.try_into().unwrap();
        let result = expr.eval().unwrap();
        assert_eq!(result, Val::Bool(false));
    }

    #[test]
    fn mixed_type_comparisons() {
        // List vs non-list
        let expr: Expr = "[1, 2, 3] == 123".try_into().unwrap();
        let result = expr.eval().unwrap();
        assert_eq!(result, Val::Bool(false));

        // Map vs non-map
        let expr: Expr = r#"{"a": 1} == 1"#.try_into().unwrap();
        let result = expr.eval().unwrap();
        assert_eq!(result, Val::Bool(false));

        // Empty structures
        let expr: Expr = "[] == {}".try_into().unwrap();
        let result = expr.eval().unwrap();
        assert_eq!(result, Val::Bool(false));
    }
}
