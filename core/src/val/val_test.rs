#[cfg(test)]
mod tests {
    use crate::val::Val;
    use std::collections::HashMap;

    macro_rules! test_op {
        ($name:ident, $op:tt, $l:expr, $r:expr, $res:expr) => {
            #[test]
            fn $name() {
                let l: Val = $l.into();
                let r: Val = $r.into();
                let res: Val = $res.into();
                assert_eq!((&l $op &r).unwrap(), res);
            }
        };
    }

    test_op!(add, +, 1, 2, 3);

    test_op!(sub, -, 1, 2, -1);

    test_op!(mul, *, 2, 3, 6);
    test_op!(div, /, 3, 2, 1.5);
    test_op!(list_add_val, +, vec![1], 2, vec![1, 2]);
    test_op!(list_add_list, +, vec![1], vec![2], vec![1, 2]);
    test_op!(list_sub_val, -, vec![1, 2], 2, vec![1]);
    test_op!(list_sub_list, -, vec![1, 2], vec![2], vec![1]);

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

        // Map operations
        #[test]
        fn map_add_map() {
            let mut map1 = HashMap::new();
            map1.insert("a", 1);
            map1.insert("b", 2);

            let mut map2 = HashMap::new();
            map2.insert("c", 3);
            map2.insert("a", 4); // This should override map1's "a"

            let mut expected = HashMap::new();
            expected.insert("a".to_string(), Val::Int(4));
            expected.insert("b".to_string(), Val::Int(2));
            expected.insert("c".to_string(), Val::Int(3));

            let l: Val = map1.into();
            let r: Val = map2.into();
            let result = (&l + &r).unwrap();

            assert_eq!(result, expected.into());
        }

        #[test]
        fn map_sub_keys() {
            let mut map1 = HashMap::new();
            map1.insert("a", 1);
            map1.insert("b", 2);
            map1.insert("c", 3);

            let mut map2 = HashMap::new();
            map2.insert("a", 10); // Value doesn't matter, only key is used in subtraction

            let mut expected = HashMap::new();
            expected.insert("b".to_string(), Val::Int(2));
            expected.insert("c".to_string(), Val::Int(3));

            let l: Val = map1.into();
            let r: Val = map2.into();
            let result = (&l - &r).unwrap();

            assert_eq!(result, expected.into());
        }

        #[test]
        fn map_sub_str_key() {
            let mut map1 = HashMap::new();
            map1.insert("a", 1);
            map1.insert("b", 2);

            let key = "a";

            let mut expected = HashMap::new();
            expected.insert("b".to_string(), Val::Int(2));

            let l: Val = map1.into();
            let r: Val = key.into();
            let result = (&l - &r).unwrap();

            assert_eq!(result, expected.into());
        }
    }

    // Access tests
    #[test]
    fn test_map_access() {
        let mut map = HashMap::new();
        map.insert("name", "alice".to_string());
        map.insert("age", 30.to_string());

        let val: Val = map.into();
        let field = Val::Str("name".into());

        assert_eq!(val.access(&field), Some(Val::Str("alice".into())));
    }

    #[test]
    fn test_list_access() {
        let list = vec![10, 20, 30];
        let val: Val = list.into();
        let index = Val::Int(1);

        assert_eq!(val.access(&index), Some(Val::Int(20)));
    }

    #[test]
    fn test_access_out_of_bounds() {
        let list = vec![10, 20, 30];
        let val: Val = list.into();
        let index = Val::Int(5);

        assert_eq!(val.access(&index), None);
    }

    #[test]
    fn test_access_negative_index() {
        let list = vec![10, 20, 30];
        let val: Val = list.into();
        let index = Val::Int(-1);

        assert_eq!(val.access(&index), None);
    }

    // Literal creation tests
    #[test]
    fn test_literal_list_creation() {
        let list = vec![Val::Int(1), Val::Str("hello".into()), Val::Bool(true)];
        let val = Val::List(list.clone().into());

        // Test access
        assert_eq!(val.access(&Val::Int(0)), Some(Val::Int(1)));
        assert_eq!(val.access(&Val::Int(1)), Some(Val::Str("hello".into())));
        assert_eq!(val.access(&Val::Int(2)), Some(Val::Bool(true)));
        assert_eq!(val.access(&Val::Int(3)), None);
    }

    #[test]
    fn test_literal_map_creation() {
        let mut map = HashMap::new();
        map.insert("name".to_string(), Val::Str("Alice".into()));
        map.insert("age".to_string(), Val::Int(30));
        map.insert("active".to_string(), Val::Bool(true));

        let val = Val::from(map);

        // Test access
        assert_eq!(val.access(&Val::Str("name".into())), Some(Val::Str("Alice".into())));
        assert_eq!(val.access(&Val::Str("age".into())), Some(Val::Int(30)));
        assert_eq!(val.access(&Val::Str("active".into())), Some(Val::Bool(true)));
        assert_eq!(val.access(&Val::Str("nonexistent".into())), None);
    }

    #[test]
    fn test_nested_literal_access() {
        // Create nested structure: {"users": [{"name": "Alice", "age": 30}]}
        let mut inner_map = HashMap::new();
        inner_map.insert("name".to_string(), Val::Str("Alice".into()));
        inner_map.insert("age".to_string(), Val::Int(30));

        let users_list = vec![Val::from(inner_map)];

        let mut outer_map = HashMap::new();
        outer_map.insert("users".to_string(), Val::List(users_list.into()));

        let val = Val::from(outer_map);

        // Test nested access
        let users = val.access(&Val::Str("users".into())).unwrap();
        let first_user = users.access(&Val::Int(0)).unwrap();
        let name = first_user.access(&Val::Str("name".into())).unwrap();

        assert_eq!(name, Val::Str("Alice".into()));
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
        let a = Val::Str("abc".into());
        let b = Val::Str("def".into());

        assert!(a < b);
    }

    #[test]
    fn test_incomparable_types() {
        let a = Val::Int(10);
        let b = Val::Str("abc".into());

        assert_eq!(a.partial_cmp(&b), None);
    }

    #[test]
    fn test_literal_equality() {
        // Test list equality
        let list1 = Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)].into());
        let list2 = Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(3)].into());
        let list3 = Val::List(vec![Val::Int(1), Val::Int(2), Val::Int(4)].into());

        assert_eq!(list1, list2);
        assert_ne!(list1, list3);

        // Test map equality
        let mut map1 = HashMap::new();
        map1.insert("a".to_string(), Val::Int(1));
        map1.insert("b".to_string(), Val::Int(2));

        let mut map2 = HashMap::new();
        map2.insert("a".to_string(), Val::Int(1));
        map2.insert("b".to_string(), Val::Int(2));

        let mut map3 = HashMap::new();
        map3.insert("a".to_string(), Val::Int(1));
        map3.insert("b".to_string(), Val::Int(3));

        let val1 = Val::from(map1);
        let val2 = Val::from(map2);
        let val3 = Val::from(map3);

        assert_eq!(val1, val2);
        assert_ne!(val1, val3);
    }

    #[test]
    fn test_display_formatting() {
        // Test list display
        let list = Val::List(vec![Val::Int(1), Val::Str("hello".into()), Val::Bool(true)].into());
        let display = format!("{}", list);
        assert!(display.contains("1") && display.contains("hello") && display.contains("true"));

        // Test map display
        let mut map = HashMap::new();
        map.insert("name".to_string(), Val::Str("Alice".into()));
        map.insert("age".to_string(), Val::Int(30));
        let val = Val::from(map);
        let display = format!("{}", val);
        assert!(
            display.contains("name") && display.contains("Alice") && display.contains("age") && display.contains("30")
        );
    }

    #[test]
    fn test_from_yaml_value() {
        // Test basic YAML value conversions
        let yaml_str = serde_yaml::Value::String("hello".to_string());
        let val: Val = yaml_str.into();
        assert_eq!(val, Val::Str("hello".into()));

        let yaml_int = serde_yaml::Value::Number(serde_yaml::Number::from(42));
        let val: Val = yaml_int.into();
        assert_eq!(val, Val::Int(42));

        let yaml_float = serde_yaml::Value::Number(serde_yaml::Number::from(std::f64::consts::PI));
        let val: Val = yaml_float.into();
        assert_eq!(val, Val::Float(std::f64::consts::PI));

        let yaml_bool = serde_yaml::Value::Bool(true);
        let val: Val = yaml_bool.into();
        assert_eq!(val, Val::Bool(true));

        let yaml_null = serde_yaml::Value::Null;
        let val: Val = yaml_null.into();
        assert_eq!(val, Val::Nil);
    }

    #[test]
    fn test_yaml_sequence() {
        let yaml_seq = serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::Number(serde_yaml::Number::from(1)),
            serde_yaml::Value::String("hello".to_string()),
            serde_yaml::Value::Bool(true),
        ]);
        let val: Val = yaml_seq.into();

        let expected = Val::List(vec![Val::Int(1), Val::Str("hello".into()), Val::Bool(true)].into());

        assert_eq!(val, expected);
    }

    #[test]
    fn test_yaml_mapping() {
        let mut yaml_map = serde_yaml::Mapping::new();
        yaml_map.insert(
            serde_yaml::Value::String("name".to_string()),
            serde_yaml::Value::String("Alice".to_string()),
        );
        yaml_map.insert(
            serde_yaml::Value::String("age".to_string()),
            serde_yaml::Value::Number(serde_yaml::Number::from(30)),
        );

        let yaml_mapping = serde_yaml::Value::Mapping(yaml_map);
        let val: Val = yaml_mapping.into();

        let mut expected_map = HashMap::new();
        expected_map.insert("name".to_string(), Val::Str("Alice".into()));
        expected_map.insert("age".to_string(), Val::Int(30));
        let expected = Val::from(expected_map);

        assert_eq!(val, expected);
    }

    #[test]
    fn test_yaml_tagged_value() {
        use serde_yaml::value::{Tag, TaggedValue};

        let tagged = serde_yaml::Value::Tagged(Box::new(TaggedValue {
            tag: Tag::new("!custom"),
            value: serde_yaml::Value::String("tagged_value".to_string()),
        }));
        let val: Val = tagged.into();
        assert_eq!(val, Val::Str("tagged_value".into()));
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

    #[test]
    fn test_parse_with_format_json() {
        use crate::val::de::{Format, parse_with_format};

        // Auto-detect JSON
        let json_input = r#"{"name": "Alice", "age": 30}"#;
        let result = parse_with_format(json_input, None).unwrap();
        assert_eq!(result.access(&Val::Str("name".into())), Some(Val::Str("Alice".into())));
        assert_eq!(result.access(&Val::Str("age".into())), Some(Val::Int(30)));

        // Force JSON format
        let json_input2 = r#"{"name": "Charlie", "age": 35}"#;
        let result = parse_with_format(json_input2, Some(Format::Json)).unwrap();
        assert_eq!(
            result.access(&Val::Str("name".into())),
            Some(Val::Str("Charlie".into()))
        );
    }

    #[test]
    fn test_parse_with_format_yaml() {
        use crate::val::de::{Format, parse_with_format};

        // Auto-detect YAML
        let yaml_input = "name: Bob\nage: 25";
        let result = parse_with_format(yaml_input, None).unwrap();
        assert_eq!(result.access(&Val::Str("name".into())), Some(Val::Str("Bob".into())));
        assert_eq!(result.access(&Val::Str("age".into())), Some(Val::Int(25)));

        // Force YAML format
        let yaml_input2 = "name: Dave\nage: 40";
        let result = parse_with_format(yaml_input2, Some(Format::Yaml)).unwrap();
        assert_eq!(result.access(&Val::Str("name".into())), Some(Val::Str("Dave".into())));
    }

    #[test]
    fn test_parse_with_format_all() {
        use crate::val::de::{Format, parse_with_format};

        // Auto-detect JSON
        let json_input = r#"{"name": "Alice", "age": 30}"#;
        let result = parse_with_format(json_input, None).unwrap();
        assert_eq!(result.access(&Val::Str("name".into())), Some(Val::Str("Alice".into())));
        assert_eq!(result.access(&Val::Str("age".into())), Some(Val::Int(30)));

        // Auto-detect YAML
        let yaml_input = "name: Bob\nage: 25";
        let result = parse_with_format(yaml_input, None).unwrap();
        assert_eq!(result.access(&Val::Str("name".into())), Some(Val::Str("Bob".into())));
        assert_eq!(result.access(&Val::Str("age".into())), Some(Val::Int(25)));

        // Force JSON format
        let yaml_as_json = r#"{"name": "Charlie", "age": 35}"#;
        let result = parse_with_format(yaml_as_json, Some(Format::Json)).unwrap();
        assert_eq!(
            result.access(&Val::Str("name".into())),
            Some(Val::Str("Charlie".into()))
        );

        // Force YAML format
        let json_as_yaml = "name: Dave\nage: 40";
        let result = parse_with_format(json_as_yaml, Some(Format::Yaml)).unwrap();
        assert_eq!(result.access(&Val::Str("name".into())), Some(Val::Str("Dave".into())));
    }
}
