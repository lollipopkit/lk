use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use crate::val::HeapValue;

use super::Val;

impl Serialize for Val {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Val::ShortStr(s) => serializer.serialize_str(s.as_str()),
            Val::Int(i) => serializer.serialize_i64(*i),
            Val::Float(f) => serializer.serialize_f64(*f),
            Val::Bool(b) => serializer.serialize_bool(*b),
            Val::Obj(value) => serialize_heap_value(value.as_ref(), serializer),
            Val::Nil => serializer.serialize_unit(),
        }
    }
}

fn serialize_heap_value<S>(value: &HeapValue, serializer: S) -> Result<S::Ok, S::Error>
where
    S: Serializer,
{
    match value {
        HeapValue::String(value) => serializer.serialize_str(value.as_ref()),
        HeapValue::List(values) => values.to_val_values().serialize(serializer),
        HeapValue::Map(values) => values.to_val_entries().serialize(serializer),
        HeapValue::Callable(_) => serializer.serialize_str("<function>"),
        HeapValue::Task(task) => {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("type", "task")?;
            map.serialize_entry("value", &format!("{:?}", task.value))?;
            map.end()
        }
        HeapValue::Channel(channel) => {
            let mut map = serializer.serialize_map(Some(3))?;
            map.serialize_entry("type", "channel")?;
            map.serialize_entry("capacity", &channel.capacity)?;
            map.serialize_entry("inner_type", &format!("{:?}", channel.inner_type))?;
            map.end()
        }
        HeapValue::Stream(stream) => {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("type", "stream")?;
            map.serialize_entry("inner_type", &format!("{:?}", stream.inner_type))?;
            map.end()
        }
        HeapValue::StreamCursor(cursor) => {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("type", "stream_cursor")?;
            map.serialize_entry("stream_id", &cursor.stream_id)?;
            map.end()
        }
        HeapValue::Object(object) => {
            let mut map = serializer.serialize_map(Some(object.fields.len() + 1))?;
            map.serialize_entry("__type", object.type_name.as_ref())?;
            for (key, value) in object.fields.iter() {
                map.serialize_entry(key.as_ref(), &Val::object_field_to_val(value))?;
            }
            map.end()
        }
        HeapValue::UpvalCell(value) => Val::object_field_to_val(value).serialize(serializer),
        HeapValue::ErrorVal(error) => {
            let mut map = serializer.serialize_map(Some(2))?;
            map.serialize_entry("message", error.message.as_ref())?;
            let trace = error.trace.iter().map(Val::object_field_to_val).collect::<Vec<_>>();
            map.serialize_entry("trace", &trace)?;
            map.end()
        }
    }
}
