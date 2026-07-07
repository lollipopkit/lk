#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
#[cfg(test)]
mod tests {
    use crate::{
        val::{LiteralVal, RuntimeVal},
        vm::execute_source,
    };
    use core::mem::size_of;

    #[test]
    fn runtime_val_stays_within_two_words() {
        assert!(
            size_of::<RuntimeVal>() <= 16,
            "RuntimeVal grew to {} bytes; keep it within two machine words for register density",
            size_of::<RuntimeVal>()
        );
    }

    fn expect_expr(expr: &str, expected: &str) {
        let result = execute_source(&format!("return {expr};")).expect("execute source");
        assert_eq!(result.display_first_return(), expected);
    }

    fn panic_expr(expr: &str) {
        assert!(execute_source(&format!("return {expr};")).is_err());
    }

    #[test]
    fn arithmetic_runs_through_exec() {
        expect_expr("1 + 2", "3");
        expect_expr("1 - 2", "-1");
        expect_expr("2 * 3", "6");
        expect_expr("3 / 2", "1.5");
    }

    #[test]
    fn string_concat_preserves_short_and_heap_string_shapes() {
        assert!(matches!(
            LiteralVal::concat_strings("ab", "cd"),
            LiteralVal::ShortStr(_)
        ));
        assert!(matches!(
            LiteralVal::concat_strings("longer-", "than-short"),
            LiteralVal::String(_)
        ));
        assert_eq!(
            LiteralVal::concat_strings("prefix-", "suffix").as_str(),
            Some("prefix-suffix")
        );
    }

    #[test]
    fn modulo_runs_through_exec() {
        expect_expr("7 % 3", "1");
        expect_expr("7.5 % 2.0", "1.5");
        expect_expr("7 % 2.5", "2");
        expect_expr("7.5 % 2", "1.5");
    }

    #[test]
    fn string_numeric_concat_runs_through_exec() {
        expect_expr(r#""hello" + 123"#, "hello123");
        expect_expr(r#""hello" + 12.34"#, "hello12.34");
        expect_expr(r#"123 + "hello""#, "123hello");
        expect_expr(r#"12.34 + "hello""#, "12.34hello");
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
    fn test_access_out_of_bounds() {
        expect_expr("[10, 20, 30].5 == nil", "true");
    }

    #[test]
    fn test_access_negative_index() {
        expect_expr("[10, 20, 30][-1]", "30");
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

    #[test]
    fn comparisons_run_through_exec() {
        expect_expr("10 < 20", "true");
        expect_expr("10.5 < 20.5", "true");
        expect_expr("10 < 10.5", "true");
        expect_expr(r#""abc" < "def""#, "true");
        panic_expr(r#"10 < "abc""#);
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
