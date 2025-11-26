#[cfg(test)]
mod tests {
    use crate::val::{Val, de::*};
    use std::sync::Arc;

    #[test]
    fn test_from_json_str_basic() {
        let json = r#"{"name": "test", "age": 25, "active": true}"#;
        let val = from_json_str(json).unwrap();

        if let Val::Map(map) = val {
            assert_eq!(map.get("name"), Some(&Val::Str(Arc::from("test"))));
            assert_eq!(map.get("age"), Some(&Val::Int(25)));
            assert_eq!(map.get("active"), Some(&Val::Bool(true)));
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_json_str_array() {
        let json = r#"[1, 2.5, "hello", true, null]"#;
        let val = from_json_str(json).unwrap();

        if let Val::List(list) = val {
            assert_eq!(list.len(), 5);
            assert_eq!(list[0], Val::Int(1));
            assert_eq!(list[1], Val::Float(2.5));
            assert_eq!(list[2], Val::Str(Arc::from("hello")));
            assert_eq!(list[3], Val::Bool(true));
            assert_eq!(list[4], Val::Nil);
        } else {
            panic!("Expected List, got {:?}", val);
        }
    }

    #[test]
    fn test_from_json_str_nested() {
        let json = r#"{"user": {"name": "admin", "permissions": ["read", "write"]}, "count": 42}"#;
        let val = from_json_str(json).unwrap();

        if let Val::Map(map) = val {
            if let Some(Val::Map(user_map)) = map.get("user") {
                assert_eq!(user_map.get("name"), Some(&Val::Str(Arc::from("admin"))));
                if let Some(Val::List(perms)) = user_map.get("permissions") {
                    assert_eq!(perms.len(), 2);
                    assert_eq!(perms[0], Val::Str(Arc::from("read")));
                    assert_eq!(perms[1], Val::Str(Arc::from("write")));
                } else {
                    panic!("Expected permissions list");
                }
            } else {
                panic!("Expected user map");
            }
            assert_eq!(map.get("count"), Some(&Val::Int(42)));
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_yaml_str_basic() {
        let yaml = r#"
name: test
age: 25
active: true
"#;
        let val = from_yaml_str(yaml).unwrap();

        if let Val::Map(map) = val {
            assert_eq!(map.get("name"), Some(&Val::Str(Arc::from("test"))));
            assert_eq!(map.get("age"), Some(&Val::Int(25)));
            assert_eq!(map.get("active"), Some(&Val::Bool(true)));
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_yaml_str_array() {
        let yaml = r#"
- 1
- 2.5
- hello
- true
- null
"#;
        let val = from_yaml_str(yaml).unwrap();

        if let Val::List(list) = val {
            assert_eq!(list.len(), 5);
            assert_eq!(list[0], Val::Int(1));
            assert_eq!(list[1], Val::Float(2.5));
            assert_eq!(list[2], Val::Str(Arc::from("hello")));
            assert_eq!(list[3], Val::Bool(true));
            assert_eq!(list[4], Val::Nil);
        } else {
            panic!("Expected List, got {:?}", val);
        }
    }

    #[test]
    fn test_from_yaml_str_nested() {
        let yaml = r#"
user:
  name: admin
  permissions:
    - read
    - write
count: 42
"#;
        let val = from_yaml_str(yaml).unwrap();

        if let Val::Map(map) = val {
            if let Some(Val::Map(user_map)) = map.get("user") {
                assert_eq!(user_map.get("name"), Some(&Val::Str(Arc::from("admin"))));
                if let Some(Val::List(perms)) = user_map.get("permissions") {
                    assert_eq!(perms.len(), 2);
                    assert_eq!(perms[0], Val::Str(Arc::from("read")));
                    assert_eq!(perms[1], Val::Str(Arc::from("write")));
                } else {
                    panic!("Expected permissions list");
                }
            } else {
                panic!("Expected user map");
            }
            assert_eq!(map.get("count"), Some(&Val::Int(42)));
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_yaml_str_multiline() {
        let yaml = r#"
description: |
  This is a multiline
  string in YAML
summary: >
  This is a folded
  string in YAML
"#;
        let val = from_yaml_str(yaml).unwrap();

        if let Val::Map(map) = val {
            if let Some(Val::Str(desc)) = map.get("description") {
                assert!(desc.contains("This is a multiline\nstring in YAML"));
            } else {
                panic!("Expected description string");
            }
            if let Some(Val::Str(summary)) = map.get("summary") {
                assert!(summary.contains("This is a folded string in YAML"));
            } else {
                panic!("Expected summary string");
            }
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_toml_str_basic() {
        let toml = r#"
name = "test"
age = 25
active = true
"#;
        let val = from_toml_str(toml).unwrap();

        if let Val::Map(map) = val {
            assert_eq!(map.get("name"), Some(&Val::Str(Arc::from("test"))));
            assert_eq!(map.get("age"), Some(&Val::Int(25)));
            assert_eq!(map.get("active"), Some(&Val::Bool(true)));
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_toml_str_array() {
        let toml = r#"
numbers = [1, 2, 3]
strings = ["hello", "world"]
mixed = [1, "hello", true]
"#;
        let val = from_toml_str(toml).unwrap();

        if let Val::Map(map) = val {
            if let Some(Val::List(numbers)) = map.get("numbers") {
                assert_eq!(numbers.len(), 3);
                assert_eq!(numbers[0], Val::Int(1));
                assert_eq!(numbers[1], Val::Int(2));
                assert_eq!(numbers[2], Val::Int(3));
            } else {
                panic!("Expected numbers array");
            }

            if let Some(Val::List(strings)) = map.get("strings") {
                assert_eq!(strings.len(), 2);
                assert_eq!(strings[0], Val::Str(Arc::from("hello")));
                assert_eq!(strings[1], Val::Str(Arc::from("world")));
            } else {
                panic!("Expected strings array");
            }

            if let Some(Val::List(mixed)) = map.get("mixed") {
                assert_eq!(mixed.len(), 3);
                assert_eq!(mixed[0], Val::Int(1));
                assert_eq!(mixed[1], Val::Str(Arc::from("hello")));
                assert_eq!(mixed[2], Val::Bool(true));
            } else {
                panic!("Expected mixed array");
            }
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_toml_str_table() {
        let toml = r#"
[user]
name = "admin"
permissions = ["read", "write"]

[database]
host = "localhost"
port = 5432
"#;
        let val = from_toml_str(toml).unwrap();

        if let Val::Map(map) = val {
            if let Some(Val::Map(user_map)) = map.get("user") {
                assert_eq!(user_map.get("name"), Some(&Val::Str(Arc::from("admin"))));
                if let Some(Val::List(perms)) = user_map.get("permissions") {
                    assert_eq!(perms.len(), 2);
                    assert_eq!(perms[0], Val::Str(Arc::from("read")));
                    assert_eq!(perms[1], Val::Str(Arc::from("write")));
                } else {
                    panic!("Expected permissions list");
                }
            } else {
                panic!("Expected user map");
            }

            if let Some(Val::Map(db_map)) = map.get("database") {
                assert_eq!(db_map.get("host"), Some(&Val::Str(Arc::from("localhost"))));
                assert_eq!(db_map.get("port"), Some(&Val::Int(5432)));
            } else {
                panic!("Expected database map");
            }
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_toml_str_nested_table() {
        let toml = r#"
[req.user]
role = "admin"
id = 123

[req.resource]
type = "document"
name = "test.txt"
"#;
        let val = from_toml_str(toml).unwrap();

        if let Val::Map(map) = val {
            if let Some(Val::Map(req_map)) = map.get("req") {
                if let Some(Val::Map(user_map)) = req_map.get("user") {
                    assert_eq!(user_map.get("role"), Some(&Val::Str(Arc::from("admin"))));
                    assert_eq!(user_map.get("id"), Some(&Val::Int(123)));
                } else {
                    panic!("Expected user map");
                }

                if let Some(Val::Map(resource_map)) = req_map.get("resource") {
                    assert_eq!(resource_map.get("type"), Some(&Val::Str(Arc::from("document"))));
                    assert_eq!(resource_map.get("name"), Some(&Val::Str(Arc::from("test.txt"))));
                } else {
                    panic!("Expected resource map");
                }
            } else {
                panic!("Expected req map");
            }
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_from_toml_str_table_array() {
        let toml = r#"
[[users]]
name = "alice"
role = "admin"

[[users]]
name = "bob"
role = "user"
"#;
        let val = from_toml_str(toml).unwrap();

        if let Val::Map(map) = val {
            if let Some(Val::List(users)) = map.get("users") {
                assert_eq!(users.len(), 2);

                if let Val::Map(alice) = &users[0] {
                    assert_eq!(alice.get("name"), Some(&Val::Str(Arc::from("alice"))));
                    assert_eq!(alice.get("role"), Some(&Val::Str(Arc::from("admin"))));
                } else {
                    panic!("Expected alice map");
                }

                if let Val::Map(bob) = &users[1] {
                    assert_eq!(bob.get("name"), Some(&Val::Str(Arc::from("bob"))));
                    assert_eq!(bob.get("role"), Some(&Val::Str(Arc::from("user"))));
                } else {
                    panic!("Expected bob map");
                }
            } else {
                panic!("Expected users array");
            }
        } else {
            panic!("Expected Map, got {:?}", val);
        }
    }

    #[test]
    fn test_detect_format_json() {
        assert_eq!(detect_format(r#"{"key": "value"}"#), Format::Json);
        assert_eq!(detect_format(r#"[1, 2, 3]"#), Format::Json);
        assert_eq!(detect_format(r#"{"nested": {"key": "value"}}"#), Format::Json);
        assert_eq!(detect_format(""), Format::Json); // Empty defaults to JSON
    }

    #[test]
    fn test_detect_format_yaml() {
        assert_eq!(detect_format("---\nkey: value"), Format::Yaml);
        assert_eq!(detect_format("key: value\nother: 123"), Format::Yaml);
        assert_eq!(detect_format("- item1\n- item2"), Format::Yaml);
        assert_eq!(detect_format("multiline: |\n  content"), Format::Yaml);
        assert_eq!(detect_format("folded: >\n  content"), Format::Yaml);
    }

    #[test]
    fn test_detect_format_toml() {
        assert_eq!(detect_format("[section]\nkey = value"), Format::Toml);
        assert_eq!(detect_format("key = \"value\""), Format::Toml);
        assert_eq!(detect_format("number = 42\nstring = \"hello\""), Format::Toml);
        assert_eq!(detect_format("[[array]]\nname = \"test\""), Format::Toml);
        assert_eq!(detect_format("nested.key = \"value\""), Format::Toml);
    }

    #[test]
    fn test_detect_format_edge_cases() {
        // JSON-like but actually YAML
        assert_eq!(detect_format("key: {\"nested\": \"value\"}"), Format::Yaml);

        // TOML with spaces around equals
        assert_eq!(detect_format("key = value"), Format::Toml);
        assert_eq!(detect_format("key=value"), Format::Toml);

        // Complex nested structures
        assert_eq!(detect_format("user:\n  name: test\n  age: 25"), Format::Yaml);
        assert_eq!(detect_format("[user]\nname = \"test\"\nage = 25"), Format::Toml);
    }

    #[test]
    fn test_parse_with_format_override() {
        let json_data = r#"{"key": "value"}"#;

        // Test JSON override
        let val = parse_with_format(json_data, Some(Format::Json)).unwrap();
        if let Val::Map(map) = val {
            assert_eq!(map.get("key"), Some(&Val::Str(Arc::from("value"))));
        } else {
            panic!("Expected Map");
        }

        // Test auto-detection
        let val = parse_with_format(json_data, None).unwrap();
        if let Val::Map(map) = val {
            assert_eq!(map.get("key"), Some(&Val::Str(Arc::from("value"))));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_parse_with_format_yaml_override() {
        let yaml_data = "key: value\nother: 123";

        // Test YAML override
        let val = parse_with_format(yaml_data, Some(Format::Yaml)).unwrap();
        if let Val::Map(map) = val {
            assert_eq!(map.get("key"), Some(&Val::Str(Arc::from("value"))));
            assert_eq!(map.get("other"), Some(&Val::Int(123)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_parse_with_format_toml_override() {
        let toml_data = "key = \"value\"\nother = 123";

        // Test TOML override
        let val = parse_with_format(toml_data, Some(Format::Toml)).unwrap();
        if let Val::Map(map) = val {
            assert_eq!(map.get("key"), Some(&Val::Str(Arc::from("value"))));
            assert_eq!(map.get("other"), Some(&Val::Int(123)));
        } else {
            panic!("Expected Map");
        }
    }

    #[test]
    fn test_error_handling() {
        // Invalid JSON
        assert!(from_json_str(r#"{"invalid": json"#).is_err());

        // Invalid YAML
        assert!(from_yaml_str("invalid: [\n  - yaml\n  - structure\n").is_err());

        // Invalid TOML
        assert!(from_toml_str("invalid = toml = syntax").is_err());
    }

    #[test]
    fn test_has_yaml_indicators() {
        assert!(has_yaml_indicators("key: value"));
        assert!(has_yaml_indicators("- item"));
        assert!(has_yaml_indicators("multiline: |"));
        assert!(has_yaml_indicators("folded: >"));
        assert!(has_yaml_indicators("# comment\nkey: value"));

        assert!(!has_yaml_indicators(r#"{"key": "value"}"#));
        assert!(!has_yaml_indicators("key = value"));
        assert!(!has_yaml_indicators(""));
    }

    #[test]
    fn test_has_toml_indicators() {
        assert!(has_toml_indicators("[section]"));
        assert!(has_toml_indicators("[[array]]"));
        assert!(has_toml_indicators("key = value"));
        assert!(has_toml_indicators("key=\"value\""));
        assert!(has_toml_indicators("nested.key = \"value\""));
        assert!(has_toml_indicators("# comment\nkey = value"));

        assert!(!has_toml_indicators(r#"{"key": "value"}"#));
        assert!(!has_toml_indicators("key: value"));
        assert!(!has_toml_indicators("- item"));
        assert!(!has_toml_indicators(""));
    }
}
