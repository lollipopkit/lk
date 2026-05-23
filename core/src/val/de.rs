use crate::val::{HeapStore, HeapValue, RuntimeMapKey, RuntimeVal, ShortStr, TypedList, TypedMap, Val};
use serde::de::{Deserialize, Deserializer, Error as DeError, Visitor};
use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

struct ValLiteralVisitor;

impl<'de> Visitor<'de> for ValLiteralVisitor {
    type Value = Val;

    fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
        formatter.write_str("an LK scalar Val literal")
    }

    fn visit_bool<E>(self, value: bool) -> Result<Val, E> {
        Ok(Val::Bool(value))
    }

    fn visit_i64<E>(self, value: i64) -> Result<Val, E> {
        Ok(Val::Int(value))
    }

    fn visit_u64<E>(self, value: u64) -> Result<Val, E>
    where
        E: DeError,
    {
        i64::try_from(value)
            .map(Val::Int)
            .map_err(|_| E::custom("u64 literal exceeds LK Int range"))
    }

    fn visit_f64<E>(self, value: f64) -> Result<Val, E> {
        Ok(Val::Float(value))
    }

    fn visit_str<E>(self, value: &str) -> Result<Val, E> {
        Ok(Val::from_str(value))
    }

    fn visit_string<E>(self, value: String) -> Result<Val, E> {
        Ok(Val::from_str(&value))
    }

    fn visit_none<E>(self) -> Result<Val, E> {
        Ok(Val::Nil)
    }

    fn visit_unit<E>(self) -> Result<Val, E> {
        Ok(Val::Nil)
    }
}

impl<'de> Deserialize<'de> for Val {
    fn deserialize<D>(deserializer: D) -> Result<Val, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ValLiteralVisitor)
    }
}

#[derive(Debug)]
pub struct RuntimeDecodedValue {
    pub value: RuntimeVal,
    pub heap: HeapStore,
}

pub fn from_json_str_runtime(input: &str) -> anyhow::Result<RuntimeDecodedValue> {
    let mut heap = HeapStore::new();
    let value = parse_runtime_with_format_into_heap(input, Format::Json, &mut heap)?;
    Ok(RuntimeDecodedValue { value, heap })
}

pub fn from_yaml_str_runtime(input: &str) -> anyhow::Result<RuntimeDecodedValue> {
    let mut heap = HeapStore::new();
    let value = parse_runtime_with_format_into_heap(input, Format::Yaml, &mut heap)?;
    Ok(RuntimeDecodedValue { value, heap })
}

pub fn from_toml_str_runtime(input: &str) -> anyhow::Result<RuntimeDecodedValue> {
    let mut heap = HeapStore::new();
    let value = parse_runtime_with_format_into_heap(input, Format::Toml, &mut heap)?;
    Ok(RuntimeDecodedValue { value, heap })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Format {
    Json,
    Yaml,
    Toml,
}

/// Automatically detect format based on content
pub fn detect_format(input: &str) -> Format {
    let trimmed = input.trim();

    // Empty input defaults to first available format
    if trimmed.is_empty() {
        return Format::Json;
    }

    // Check for obvious JSON markers
    if (trimmed.starts_with('{') && trimmed.ends_with('}')) || (trimmed.starts_with('[') && trimmed.ends_with(']')) {
        return Format::Json;
    }

    // Check for obvious YAML markers
    if trimmed.contains("---") ||  // YAML document separator
       trimmed.contains("...") ||  // YAML document end
       has_yaml_indicators(trimmed)
    {
        return Format::Yaml;
    }

    // Check for obvious TOML markers
    if has_toml_indicators(trimmed) {
        return Format::Toml;
    }

    // Try parsing as JSON first (faster and more common)
    if serde_json::from_str::<serde_json::Value>(input).is_ok() {
        return Format::Json;
    }

    // Try parsing as YAML
    if serde_yaml::from_str::<serde_yaml::Value>(input).is_ok() {
        return Format::Yaml;
    }

    // Try parsing as TOML
    if toml::from_str::<toml::Value>(input).is_ok() {
        return Format::Toml;
    }

    // Default to first available format if all fail
    Format::Json
}

/// Check for YAML-specific indicators
pub fn has_yaml_indicators(input: &str) -> bool {
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Look for YAML key-value patterns without quotes
        if trimmed.contains(':') && !trimmed.starts_with('"') && !trimmed.starts_with('{') {
            // Check if it's a YAML-style key: value (not JSON "key": value)
            if let Some(colon_pos) = trimmed.find(':') {
                let key_part = &trimmed[..colon_pos];
                // YAML keys often don't have quotes and can contain spaces/special chars
                if !key_part.starts_with('"') && !key_part.starts_with('\'') {
                    return true;
                }
            }
        }

        // Look for YAML list indicators
        if trimmed.starts_with("- ") || trimmed.starts_with("-\t") {
            return true;
        }

        // Look for YAML multi-line indicators
        if trimmed.ends_with("|") || trimmed.ends_with(">") {
            return true;
        }
    }

    false
}

/// Check for TOML-specific indicators
pub fn has_toml_indicators(input: &str) -> bool {
    for line in input.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }

        // Look for TOML section headers [section]
        if trimmed.starts_with('[') && trimmed.ends_with(']') && trimmed.len() > 2 {
            return true;
        }

        // Look for TOML table arrays [[table]]
        if trimmed.starts_with("[[") && trimmed.ends_with("]]") && trimmed.len() > 4 {
            return true;
        }

        // Look for TOML key = value patterns (with equals sign)
        if trimmed.contains(" = ") || trimmed.contains("=") {
            // Check if it's a simple key = value pattern
            if let Some(eq_pos) = trimmed.find('=') {
                let key_part = trimmed[..eq_pos].trim();
                let value_part = trimmed[eq_pos + 1..].trim();

                // TOML keys are usually unquoted identifiers or quoted strings
                // Values can be strings, numbers, booleans, arrays, etc.
                if !key_part.is_empty() && !value_part.is_empty() {
                    // Check if key looks like a TOML identifier
                    if key_part
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-' || c == '.')
                        || (key_part.starts_with('"') && key_part.ends_with('"'))
                        || (key_part.starts_with('\'') && key_part.ends_with('\''))
                    {
                        return true;
                    }
                }
            }
        }
    }

    false
}

pub fn parse_runtime_with_format(input: &str, format_override: Option<Format>) -> anyhow::Result<RuntimeDecodedValue> {
    let format = format_override.unwrap_or_else(|| detect_format(input));
    let mut heap = HeapStore::new();
    let value = parse_runtime_with_format_into_heap(input, format, &mut heap)?;
    Ok(RuntimeDecodedValue { value, heap })
}

pub fn parse_runtime_with_format_into_heap(
    input: &str,
    format: Format,
    heap: &mut HeapStore,
) -> anyhow::Result<RuntimeVal> {
    match format {
        Format::Json => {
            let value = serde_json::from_str::<serde_json::Value>(input).map_err(|e| anyhow::anyhow!(e))?;
            json_to_runtime(value, heap)
        }
        Format::Yaml => {
            let value = serde_yaml::from_str::<serde_yaml::Value>(input).map_err(|e| anyhow::anyhow!(e))?;
            yaml_to_runtime(value, heap)
        }
        Format::Toml => {
            let value = toml::from_str::<toml::Value>(input).map_err(|e| anyhow::anyhow!(e))?;
            toml_to_runtime(value, heap)
        }
    }
}

fn json_to_runtime(value: serde_json::Value, heap: &mut HeapStore) -> anyhow::Result<RuntimeVal> {
    Ok(match value {
        serde_json::Value::Null => RuntimeVal::Nil,
        serde_json::Value::Bool(value) => RuntimeVal::Bool(value),
        serde_json::Value::Number(value) => number_to_runtime(value.as_i64(), value.as_f64()),
        serde_json::Value::String(value) => runtime_string_value(&value, heap),
        serde_json::Value::Array(values) => {
            let values = values
                .into_iter()
                .map(|value| json_to_runtime(value, heap))
                .collect::<anyhow::Result<Vec<_>>>()?;
            RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::from_runtime_values(values, heap))))
        }
        serde_json::Value::Object(values) => {
            let entries = values
                .into_iter()
                .map(|(key, value)| Ok((runtime_string_key(&key), json_to_runtime(value, heap)?)))
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
            RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))))
        }
    })
}

fn yaml_to_runtime(value: serde_yaml::Value, heap: &mut HeapStore) -> anyhow::Result<RuntimeVal> {
    Ok(match value {
        serde_yaml::Value::Null => RuntimeVal::Nil,
        serde_yaml::Value::Bool(value) => RuntimeVal::Bool(value),
        serde_yaml::Value::Number(value) => number_to_runtime(value.as_i64(), value.as_f64()),
        serde_yaml::Value::String(value) => runtime_string_value(&value, heap),
        serde_yaml::Value::Sequence(values) => {
            let values = values
                .into_iter()
                .map(|value| yaml_to_runtime(value, heap))
                .collect::<anyhow::Result<Vec<_>>>()?;
            RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::from_runtime_values(values, heap))))
        }
        serde_yaml::Value::Mapping(values) => {
            let entries = values
                .into_iter()
                .map(|(key, value)| Ok((yaml_key_to_runtime(key)?, yaml_to_runtime(value, heap)?)))
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
            RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))))
        }
        serde_yaml::Value::Tagged(value) => yaml_to_runtime(value.value, heap)?,
    })
}

fn toml_to_runtime(value: toml::Value, heap: &mut HeapStore) -> anyhow::Result<RuntimeVal> {
    Ok(match value {
        toml::Value::String(value) => runtime_string_value(&value, heap),
        toml::Value::Integer(value) => RuntimeVal::Int(value),
        toml::Value::Float(value) => RuntimeVal::Float(value),
        toml::Value::Boolean(value) => RuntimeVal::Bool(value),
        toml::Value::Datetime(value) => runtime_string_value(&value.to_string(), heap),
        toml::Value::Array(values) => {
            let values = values
                .into_iter()
                .map(|value| toml_to_runtime(value, heap))
                .collect::<anyhow::Result<Vec<_>>>()?;
            RuntimeVal::Obj(heap.alloc(HeapValue::List(TypedList::from_runtime_values(values, heap))))
        }
        toml::Value::Table(values) => {
            let entries = values
                .into_iter()
                .map(|(key, value)| Ok((runtime_string_key(&key), toml_to_runtime(value, heap)?)))
                .collect::<anyhow::Result<BTreeMap<_, _>>>()?;
            RuntimeVal::Obj(heap.alloc(HeapValue::Map(TypedMap::from_runtime_entries(entries))))
        }
    })
}

fn number_to_runtime(int_value: Option<i64>, float_value: Option<f64>) -> RuntimeVal {
    int_value
        .map(RuntimeVal::Int)
        .or_else(|| float_value.map(RuntimeVal::Float))
        .unwrap_or(RuntimeVal::Nil)
}

fn runtime_string_value(value: &str, heap: &mut HeapStore) -> RuntimeVal {
    if let Some(value) = ShortStr::new(value) {
        RuntimeVal::ShortStr(value)
    } else {
        RuntimeVal::Obj(heap.alloc(HeapValue::String(Arc::<str>::from(value))))
    }
}

fn runtime_string_key(value: &str) -> RuntimeMapKey {
    if let Some(value) = ShortStr::new(value) {
        RuntimeMapKey::ShortStr(value)
    } else {
        RuntimeMapKey::String(Arc::<str>::from(value))
    }
}

fn yaml_key_to_runtime(value: serde_yaml::Value) -> anyhow::Result<RuntimeMapKey> {
    Ok(match value {
        serde_yaml::Value::Null => RuntimeMapKey::Nil,
        serde_yaml::Value::Bool(value) => RuntimeMapKey::Bool(value),
        serde_yaml::Value::Number(value) => RuntimeMapKey::Int(
            value
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("YAML map keys cannot be floats"))?,
        ),
        serde_yaml::Value::String(value) => runtime_string_key(&value),
        other => return Err(anyhow::anyhow!("YAML map key {:?} is not supported", other)),
    })
}
