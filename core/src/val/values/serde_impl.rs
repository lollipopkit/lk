use serde::ser::SerializeMap;
use serde::{Serialize, Serializer};

use super::Val;

impl Serialize for Val {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            Val::ShortStr(s) => serializer.serialize_str(s.as_str()),
            Val::Str(s) => serializer.serialize_str(s.as_ref()),
            Val::Int(i) => serializer.serialize_i64(*i),
            Val::Float(f) => serializer.serialize_f64(*f),
            Val::Bool(b) => serializer.serialize_bool(*b),
            Val::Map(m) => (**m).serialize(serializer),
            Val::List(l) => (**l).serialize(serializer),
            Val::Closure(_)
            | Val::RustFunction(_)
            | Val::RustFastFunction(_)
            | Val::RustFastFunctionNamed(_)
            | Val::RustFunctionNamed(_)
            | Val::AotFunction(_) => serializer.serialize_str("<function>"),
            Val::Iterator(_) => serializer.serialize_str("<iterator>"),
            Val::MutationGuard(_) => serializer.serialize_str("<mutation-guard>"),
            Val::Task(task) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "task")?;
                map.serialize_entry("value", &task.value)?;
                map.end()
            }
            Val::Channel(channel) => {
                let mut map = serializer.serialize_map(Some(3))?;
                map.serialize_entry("type", "channel")?;
                map.serialize_entry("capacity", &channel.capacity)?;
                map.serialize_entry("inner_type", &format!("{:?}", channel.inner_type))?;
                map.end()
            }
            Val::Stream(stream) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "stream")?;
                map.serialize_entry("inner_type", &format!("{:?}", stream.inner_type))?;
                map.end()
            }
            Val::StreamCursor(cursor) => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("type", "stream_cursor")?;
                map.serialize_entry("stream_id", &cursor.stream_id)?;
                map.end()
            }
            Val::Object(object) => {
                let mut map = serializer.serialize_map(Some(object.fields.len() + 1))?;
                map.serialize_entry("__type", object.type_name.as_str())?;
                for (key, value) in object.fields.iter() {
                    map.serialize_entry(key, value)?;
                }
                map.end()
            }
            Val::Nil => serializer.serialize_unit(),
        }
    }
}
