//! Helpers for converting between high-level values and their encoded LLVM
//! register representation.
//!
//! The backend keeps integers in their native two's-complement form for
//! efficient arithmetic. To distinguish `nil` and boolean literals from
//! ordinary integers (including zero), we reserve a small portion of the i64
//! range near `i64::MIN` for sentinel values. This allows the IR to perform
//! identity checks for `nil`/truthiness without losing direct integer math.

use anyhow::{Result, anyhow};

use crate::val::RuntimeVal;

/// Encoded value representing `nil`.
pub const NIL_VALUE: i64 = i64::MIN;

/// Encoded value representing boolean `false`.
pub const BOOL_FALSE_VALUE: i64 = i64::MIN + 1;

/// Encoded value representing boolean `true`.
pub const BOOL_TRUE_VALUE: i64 = i64::MIN + 2;

/// Returns `true` when the encoded value lies inside the reserved sentinel range.
#[inline]
pub const fn is_reserved_sentinel(value: i64) -> bool {
    value <= BOOL_TRUE_VALUE
}

/// Convert a runtime value into its raw encoded form. Currently supports
/// immediates (`Int`, `Bool`, `Nil`); heap and non-integer scalar values stay
/// in the runtime helper handle table.
pub fn encode_immediate(val: &RuntimeVal) -> Result<i64> {
    match val {
        RuntimeVal::Nil => Ok(NIL_VALUE),
        RuntimeVal::Bool(flag) => Ok(if *flag { BOOL_TRUE_VALUE } else { BOOL_FALSE_VALUE }),
        RuntimeVal::Int(int) => {
            if is_reserved_sentinel(*int) {
                Err(anyhow!(
                    "integer literal {} conflicts with reserved sentinel range [{}, {}]",
                    int,
                    NIL_VALUE,
                    BOOL_TRUE_VALUE
                ))
            } else {
                Ok(*int)
            }
        }
        other => Err(anyhow!(
            "value of type {} cannot be encoded as an immediate LLVM literal",
            runtime_type_name(other)
        )),
    }
}

/// Convert a raw encoded value produced by the LLVM backend back into a `RuntimeVal`
/// representing the corresponding immediate literal.
#[inline]
pub fn decode_immediate(value: i64) -> RuntimeVal {
    if value == NIL_VALUE {
        RuntimeVal::Nil
    } else if value == BOOL_FALSE_VALUE {
        RuntimeVal::Bool(false)
    } else if value == BOOL_TRUE_VALUE {
        RuntimeVal::Bool(true)
    } else {
        RuntimeVal::Int(value)
    }
}

fn runtime_type_name(value: &RuntimeVal) -> &'static str {
    match value {
        RuntimeVal::Nil => "Nil",
        RuntimeVal::Bool(_) => "Bool",
        RuntimeVal::Int(_) => "Int",
        RuntimeVal::Float(_) => "Float",
        RuntimeVal::ShortStr(_) => "String",
        RuntimeVal::Obj(_) => "Object",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_nil_and_bools() {
        assert_eq!(encode_immediate(&RuntimeVal::Nil).unwrap(), NIL_VALUE);
        assert_eq!(encode_immediate(&RuntimeVal::Bool(false)).unwrap(), BOOL_FALSE_VALUE);
        assert_eq!(encode_immediate(&RuntimeVal::Bool(true)).unwrap(), BOOL_TRUE_VALUE);
    }

    #[test]
    fn rejects_reserved_integers() {
        for reserved in [NIL_VALUE, BOOL_FALSE_VALUE, BOOL_TRUE_VALUE] {
            let err = encode_immediate(&RuntimeVal::Int(reserved)).unwrap_err();
            assert!(err.to_string().contains("conflicts with reserved sentinel"));
        }
    }

    #[test]
    fn encodes_regular_integers() {
        assert_eq!(encode_immediate(&RuntimeVal::Int(0)).unwrap(), 0);
        assert_eq!(encode_immediate(&RuntimeVal::Int(123)).unwrap(), 123);
    }

    #[test]
    fn decodes_immediates() {
        assert!(matches!(decode_immediate(NIL_VALUE), RuntimeVal::Nil));
        assert!(matches!(decode_immediate(BOOL_FALSE_VALUE), RuntimeVal::Bool(false)));
        assert!(matches!(decode_immediate(BOOL_TRUE_VALUE), RuntimeVal::Bool(true)));
        assert!(matches!(decode_immediate(42), RuntimeVal::Int(42)));
    }
}
