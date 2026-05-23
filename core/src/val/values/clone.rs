use super::Val;
use crate::vm::analysis::record_val_clone;

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
            Val::Nil => {
                record_val_clone(false);
                Val::Nil
            }
            Val::LongStr(value) => {
                record_val_clone(true);
                Val::LongStr(value.clone())
            }
        }
    }
}
