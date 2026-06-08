#[cfg(test)]
mod tests {
    use crate::val::{HeapStore, HeapValue, RuntimeVal, ShortStr, TypedList, TypedMap, de::*};

    fn short(value: &str) -> RuntimeVal {
        RuntimeVal::ShortStr(ShortStr::new(value).expect("short string"))
    }

    fn root_map(decoded: &RuntimeDecodedValue) -> &TypedMap {
        let RuntimeVal::Obj(root) = decoded.value else {
            panic!("expected root map object");
        };
        let Some(HeapValue::Map(map)) = decoded.heap.get(root) else {
            panic!("expected heap map");
        };
        map
    }

    fn heap_list(heap: &HeapStore, value: RuntimeVal) -> &TypedList {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected list heap object");
        };
        let Some(HeapValue::List(list)) = heap.get(handle) else {
            panic!("expected heap list");
        };
        list
    }

    #[test]
    fn json_decodes_to_runtime_heap_values() {
        let decoded = from_json_str_runtime(r#"{"items": [1, 2, 3], "name": "tool"}"#).unwrap();
        let map = root_map(&decoded);

        assert!(matches!(map, TypedMap::StringMixed(_)));
        assert_eq!(map.get_str("name"), Some(short("tool")));

        let list = heap_list(&decoded.heap, map.get_str("items").expect("items entry"));
        assert!(matches!(list, TypedList::Int(values) if values == &vec![1, 2, 3]));
    }

    #[test]
    fn yaml_decodes_nested_runtime_containers() {
        let decoded = from_yaml_str_runtime(
            r#"
user:
  name: admin
  permissions:
    - read
    - write
count: 42
"#,
        )
        .unwrap();
        let map = root_map(&decoded);
        assert_eq!(map.get_str("count"), Some(RuntimeVal::Int(42)));

        let RuntimeVal::Obj(user) = map.get_str("user").expect("user entry") else {
            panic!("expected user map");
        };
        let Some(HeapValue::Map(user_map)) = decoded.heap.get(user) else {
            panic!("expected user heap map");
        };
        assert_eq!(user_map.get_str("name"), Some(short("admin")));

        let perms = heap_list(
            &decoded.heap,
            user_map.get_str("permissions").expect("permissions entry"),
        );
        assert!(matches!(
            perms,
            TypedList::String(values)
                if values.as_slice() == [std::sync::Arc::<str>::from("read"), std::sync::Arc::<str>::from("write")]
        ));
    }

    #[test]
    fn toml_decodes_tables_and_arrays_to_runtime_heap_values() {
        let decoded = from_toml_str_runtime(
            r#"
[req.user]
role = "admin"
id = 123

[req.resource]
type = "document"
tags = ["read", "write"]
"#,
        )
        .unwrap();
        let map = root_map(&decoded);

        let RuntimeVal::Obj(req) = map.get_str("req").expect("req entry") else {
            panic!("expected req map");
        };
        let Some(HeapValue::Map(req_map)) = decoded.heap.get(req) else {
            panic!("expected req heap map");
        };
        let RuntimeVal::Obj(user) = req_map.get_str("user").expect("user entry") else {
            panic!("expected user map");
        };
        let Some(HeapValue::Map(user_map)) = decoded.heap.get(user) else {
            panic!("expected user heap map");
        };
        assert_eq!(user_map.get_str("id"), Some(RuntimeVal::Int(123)));
    }

    #[test]
    fn parse_runtime_with_format_detects_and_overrides_formats() {
        let json = parse_runtime_with_format(r#"{"key": "value"}"#, None).unwrap();
        assert_eq!(root_map(&json).get_str("key"), Some(short("value")));

        let yaml = parse_runtime_with_format("key: value\nother: 123", Some(Format::Yaml)).unwrap();
        let map = root_map(&yaml);
        assert_eq!(map.get_str("key"), Some(short("value")));
        assert_eq!(map.get_str("other"), Some(RuntimeVal::Int(123)));

        let toml = parse_runtime_with_format("key = \"value\"\nother = 123", Some(Format::Toml)).unwrap();
        let map = root_map(&toml);
        assert_eq!(map.get_str("key"), Some(short("value")));
        assert_eq!(map.get_str("other"), Some(RuntimeVal::Int(123)));
    }

    #[test]
    fn parse_runtime_with_format_into_existing_heap() {
        let mut heap = HeapStore::new();
        let value = parse_runtime_with_format_into_heap("[1, 2, 3]", Format::Json, &mut heap).unwrap();
        let list = heap_list(&heap, value);
        assert!(matches!(list, TypedList::Int(values) if values == &vec![1, 2, 3]));
    }

    #[test]
    fn invalid_inputs_return_errors() {
        assert!(from_json_str_runtime(r#"{"invalid": json"#).is_err());
        assert!(from_yaml_str_runtime("invalid: [\n  - yaml\n  - structure\n").is_err());
        assert!(from_toml_str_runtime("invalid = toml = syntax").is_err());
    }

    #[test]
    fn detects_formats() {
        assert_eq!(detect_format(r#"{"key": "value"}"#), Format::Json);
        assert_eq!(detect_format(r#"[1, 2, 3]"#), Format::Json);
        assert_eq!(detect_format(""), Format::Json);
        assert_eq!(detect_format("---\nkey: value"), Format::Yaml);
        assert_eq!(detect_format("- item1\n- item2"), Format::Yaml);
        assert_eq!(detect_format("multiline: |\n  content"), Format::Yaml);
        assert_eq!(detect_format("[section]\nkey = value"), Format::Toml);
        assert_eq!(detect_format("key = \"value\""), Format::Toml);
        assert_eq!(detect_format("[[array]]\nname = \"test\""), Format::Toml);
    }

    #[test]
    fn detects_yaml_and_toml_indicators() {
        assert!(has_yaml_indicators("key: value"));
        assert!(has_yaml_indicators("- item"));
        assert!(has_yaml_indicators("multiline: |"));
        assert!(!has_yaml_indicators(r#"{"key": "value"}"#));

        assert!(has_toml_indicators("[section]"));
        assert!(has_toml_indicators("[[array]]"));
        assert!(has_toml_indicators("key = value"));
        assert!(!has_toml_indicators("key: value"));
    }
}
