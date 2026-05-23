#[cfg(test)]
mod tests {
    use crate::{
        val::{Type, Val},
        vm::execute_source32,
    };
    use std::collections::HashMap;
    use std::mem::size_of;

    #[test]
    fn val_stays_within_two_words() {
        assert!(
            size_of::<Val>() <= 16,
            "Val grew to {} bytes; keep it within two machine words for register density",
            size_of::<Val>()
        );
    }

    macro_rules! test_op {
        ($name:ident, $op:tt, $l:expr, $r:expr, $res:expr) => {
            #[test]
            fn $name() {
                let l = Val::test_from($l);
                let r = Val::test_from($r);
                let res = Val::test_from($res);
                assert_eq!((&l $op &r).unwrap(), res);
            }
        };
    }

    fn expect_expr(expr: &str, expected: &str) {
        let result = execute_source32(&format!("return {expr};")).expect("execute source");
        assert_eq!(result.display_first_return(), expected);
    }

    fn panic_expr(expr: &str) {
        assert!(execute_source32(&format!("return {expr};")).is_err());
    }

    test_op!(add, +, 1, 2, 3);

    test_op!(sub, -, 1, 2, -1);

    test_op!(mul, *, 2, 3, 6);
    test_op!(div, /, 3, 2, 1.5);

    #[test]
    fn old_val_container_arithmetic_is_not_supported() {
        let list = Val::test_from(vec![1]);
        let map = Val::test_string_map_from_hashmap(HashMap::from([("answer", 42)]));

        assert!((&list + &Val::Int(2)).is_err());
        assert!((&list - &Val::Int(1)).is_err());
        assert!((&map + &map).is_err());
        assert!((&map - &Val::from_str("answer")).is_err());
    }

    #[test]
    fn heap_val_containers_satisfy_container_types() {
        let list = Val::test_list_from_values(vec![Val::Int(1)]);
        let map = Val::test_string_map_from_hashmap(HashMap::from([("answer".to_string(), Val::Int(42))]));

        assert_eq!(list.dispatch_type(), Type::List(Box::new(Type::Any)));
        assert_eq!(map.dispatch_type(), Type::Map(Box::new(Type::Any), Box::new(Type::Any)));
        assert!(Type::List(Box::new(Type::Int)).validate(&list).is_ok());
        assert!(
            Type::Map(Box::new(Type::String), Box::new(Type::Int))
                .validate(&map)
                .is_ok()
        );
    }

    #[test]
    fn string_concat_preserves_short_and_heap_string_shapes() {
        assert!(matches!(Val::concat_strings("ab", "cd"), Val::ShortStr(_)));
        assert!(matches!(Val::concat_strings("longer-", "than-short"), Val::Obj(_)));
        assert_eq!(Val::concat_strings("prefix-", "suffix").as_str(), Some("prefix-suffix"));
    }

    // Modulo tests
    test_op!(mod_int, %, 7, 3, 1);
    test_op!(mod_float, %, 7.5, 2.0, 1.5);
    test_op!(mod_mixed1, %, 7, 2.5, 2.0);
    test_op!(mod_mixed2, %, 7.5, 2, 1.5);

    mod adv_arith_tests {
        use super::*;

        // String concatenation with numbers
        test_op!(str_add_int, +, "hello", 123, "hello123");
        test_op!(str_add_float, +, "hello", 12.34, "hello12.34");
        test_op!(int_add_str, +, 123, "hello", "123hello");
        test_op!(float_add_str, +, 12.34, "hello", "12.34hello");
    }

    // Access tests
    #[test]
    fn test_map_access() {
        expect_expr(r#"{"name": "alice", "age": "30"}.name"#, "alice");
    }

    #[test]
    fn test_list_access() {
        expect_expr("[10, 20, 30].1", "20");
    }

    #[test]
    fn test_ascii_string_access_reuses_cached_single_char_arc() {
        let val: Val = "abc".into();
        let first = val.access(&Val::Int(1)).expect("first access");
        let second = val.access(&Val::Int(1)).expect("second access");

        // Single-char ASCII strings are now ShortStr (zero heap, Copy), no Arc needed.
        assert_eq!(first, Val::from_str("b"));
        assert_eq!(second, Val::from_str("b"));
        assert_eq!(first, second);
    }

    #[test]
    fn test_access_out_of_bounds() {
        expect_expr("[10, 20, 30].5 == nil", "true");
    }

    #[test]
    fn test_access_negative_index() {
        panic_expr("[10, 20, 30][-1]");
    }

    // Literal creation tests
    #[test]
    fn test_literal_list_creation() {
        expect_expr(r#"[1, "hello", true].0"#, "1");
        expect_expr(r#"[1, "hello", true].1"#, "hello");
        expect_expr(r#"[1, "hello", true].2"#, "true");
        expect_expr(r#"[1, "hello", true].3 == nil"#, "true");
    }

    #[test]
    fn test_literal_map_creation() {
        expect_expr(r#"{"name": "Alice", "age": 30, "active": true}.name"#, "Alice");
        expect_expr(r#"{"name": "Alice", "age": 30, "active": true}.age"#, "30");
        expect_expr(r#"{"name": "Alice", "age": 30, "active": true}.active"#, "true");
        expect_expr(
            r#"{"name": "Alice", "age": 30, "active": true}.nonexistent == nil"#,
            "true",
        );
    }

    #[test]
    fn test_nested_literal_access() {
        expect_expr(r#"{"users": [{"name": "Alice", "age": 30}]}.users.0.name"#, "Alice");
    }

    // Comparison tests
    #[test]
    fn test_partial_ord_integers() {
        let a = Val::Int(10);
        let b = Val::Int(20);

        assert!(a < b);
    }

    #[test]
    fn test_partial_ord_floats() {
        let a = Val::Float(10.5);
        let b = Val::Float(20.5);

        assert!(a < b);
    }

    #[test]
    fn test_partial_ord_mixed() {
        let a = Val::Int(10);
        let b = Val::Float(10.5);

        assert!(a < b);
    }

    #[test]
    fn test_partial_ord_strings() {
        let a = Val::from_str("abc");
        let b = Val::from_str("def");

        assert!(a < b);
    }

    #[test]
    fn test_incomparable_types() {
        let a = Val::Int(10);
        let b = Val::from_str("abc");

        assert_eq!(a.partial_cmp(&b), None);
    }

    #[test]
    fn test_literal_equality() {
        expect_expr("[1, 2, 3] == [1, 2, 3]", "true");
        expect_expr("[1, 2, 3] != [1, 2, 4]", "true");
        expect_expr(r#"{"a": 1, "b": 2} == {"a": 1, "b": 2}"#, "true");
        expect_expr(r#"{"a": 1, "b": 2} != {"a": 1, "b": 3}"#, "true");
    }

    #[test]
    fn test_display_formatting() {
        expect_expr(r#"[1, "hello", true]"#, "[1, hello, true]");
        expect_expr(r#"{"name": "Alice", "age": 30}.name"#, "Alice");
    }

    #[test]
    fn test_format_detection_json() {
        use crate::val::de::{Format, detect_format};

        // JSON detection
        assert_eq!(detect_format(r#"{"key": "value"}"#), Format::Json);
        assert_eq!(detect_format(r#"[1, 2, 3]"#), Format::Json);
        assert_eq!(detect_format(r#"{"nested": {"key": "value"}}"#), Format::Json);

        // Edge cases
        assert_eq!(detect_format(""), Format::Json); // Empty defaults to JSON
        assert_eq!(detect_format("   "), Format::Json); // Whitespace defaults to JSON
        assert_eq!(detect_format("null"), Format::Json); // Valid JSON
        assert_eq!(detect_format("true"), Format::Json); // Valid JSON
        assert_eq!(detect_format("42"), Format::Json); // Valid JSON
    }

    #[test]
    fn test_format_detection_yaml() {
        use crate::val::de::{Format, detect_format};

        // YAML detection
        assert_eq!(detect_format("key: value"), Format::Yaml);
        assert_eq!(detect_format("- item1\n- item2"), Format::Yaml);
        assert_eq!(detect_format("---\nkey: value"), Format::Yaml);
        assert_eq!(detect_format("key: value\n..."), Format::Yaml);
        assert_eq!(detect_format("multiline: |\n  line1\n  line2"), Format::Yaml);
        assert_eq!(detect_format("folded: >\n  line1\n  line2"), Format::Yaml);

        // Complex YAML
        assert_eq!(detect_format("person:\n  name: John\n  age: 30"), Format::Yaml);
        assert_eq!(detect_format("# Comment\nkey: value"), Format::Yaml);
    }

    #[test]
    fn test_format_detection_all() {
        use crate::val::de::{Format, detect_format};

        // JSON detection
        assert_eq!(detect_format(r#"{"key": "value"}"#), Format::Json);
        assert_eq!(detect_format(r#"[1, 2, 3]"#), Format::Json);
        assert_eq!(detect_format(r#"{"nested": {"key": "value"}}"#), Format::Json);

        // YAML detection
        assert_eq!(detect_format("key: value"), Format::Yaml);
        assert_eq!(detect_format("- item1\n- item2"), Format::Yaml);
        assert_eq!(detect_format("---\nkey: value"), Format::Yaml);
        assert_eq!(detect_format("key: value\n..."), Format::Yaml);
        assert_eq!(detect_format("multiline: |\n  line1\n  line2"), Format::Yaml);
        assert_eq!(detect_format("folded: >\n  line1\n  line2"), Format::Yaml);

        // Complex YAML
        assert_eq!(detect_format("person:\n  name: John\n  age: 30"), Format::Yaml);
        assert_eq!(detect_format("# Comment\nkey: value"), Format::Yaml);

        // Edge cases
        assert_eq!(detect_format(""), Format::Json); // Empty defaults to JSON
        assert_eq!(detect_format("   "), Format::Json); // Whitespace defaults to JSON
        assert_eq!(detect_format("null"), Format::Json); // Valid JSON
        assert_eq!(detect_format("true"), Format::Json); // Valid JSON
        assert_eq!(detect_format("42"), Format::Json); // Valid JSON
    }
}
