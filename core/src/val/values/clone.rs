use super::Val;
use crate::vm::record_val_clone;

impl Clone for Val {
    #[inline]
    fn clone(&self) -> Self {
        match self {
            Val::ShortStr(value) => {
                record_val_clone(false);
                Val::ShortStr(*value)
            }
            Val::Int(value) => {
                record_val_clone(false);
                Val::Int(*value)
            }
            Val::Float(value) => {
                record_val_clone(false);
                Val::Float(*value)
            }
            Val::Bool(value) => {
                record_val_clone(false);
                Val::Bool(*value)
            }
            Val::RustFunction(value) => {
                record_val_clone(false);
                Val::RustFunction(*value)
            }
            Val::RustFastFunction(value) => {
                record_val_clone(false);
                Val::RustFastFunction(*value)
            }
            Val::RustFastFunctionNamed(value) => {
                record_val_clone(false);
                Val::RustFastFunctionNamed(*value)
            }
            Val::RustFunctionNamed(value) => {
                record_val_clone(false);
                Val::RustFunctionNamed(*value)
            }
            Val::Nil => {
                record_val_clone(false);
                Val::Nil
            }
            Val::Str(value) => {
                record_val_clone(true);
                Val::Str(value.clone())
            }
            Val::Map(value) => {
                record_val_clone(true);
                Val::Map(value.clone())
            }
            Val::List(value) => {
                record_val_clone(true);
                Val::List(value.clone())
            }
            Val::Closure(value) => {
                record_val_clone(true);
                Val::Closure(value.clone())
            }
            Val::AotFunction(value) => {
                record_val_clone(true);
                Val::AotFunction(value.clone())
            }
            Val::Task(value) => {
                record_val_clone(true);
                Val::Task(value.clone())
            }
            Val::Channel(value) => {
                record_val_clone(true);
                Val::Channel(value.clone())
            }
            Val::Stream(value) => {
                record_val_clone(true);
                Val::Stream(value.clone())
            }
            Val::Iterator(value) => {
                record_val_clone(true);
                Val::Iterator(value.clone())
            }
            Val::MutationGuard(value) => {
                record_val_clone(true);
                Val::MutationGuard(value.clone())
            }
            Val::StreamCursor(value) => {
                record_val_clone(true);
                Val::StreamCursor(value.clone())
            }
            Val::Object(value) => {
                record_val_clone(true);
                Val::Object(value.clone())
            }
        }
    }
}
