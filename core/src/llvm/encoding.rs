//! Helpers for converting between high-level values and their encoded LLVM
//! register representation.
//!
//! The backend keeps integers in their native two's-complement form for
//! efficient arithmetic. To distinguish `nil` and boolean literals from
//! ordinary integers (including zero), we reserve a small portion of the i64
//! range near `i64::MIN` for sentinel values. This allows the IR to perform
//! identity checks for `nil`/truthiness without losing direct integer math.

use anyhow::{Result, anyhow};

use crate::val::Val;

/// Encoded value representing `nil`.
pub const NIL_VALUE: i64 = i64::MIN;

/// Encoded value representing boolean `false`.
pub const BOOL_FALSE_VALUE: i64 = i64::MIN + 1;

/// Encoded value representing boolean `true`.
pub const BOOL_TRUE_VALUE: i64 = i64::MIN + 2;

/// LLVM IR literal for `nil`.
pub const NIL_LITERAL: &str = "-9223372036854775808";

/// LLVM IR literal for boolean `false`.
pub const BOOL_FALSE_LITERAL: &str = "-9223372036854775807";

/// LLVM IR literal for boolean `true`.
pub const BOOL_TRUE_LITERAL: &str = "-9223372036854775806";

#[inline]
pub const fn bool_literal(value: bool) -> &'static str {
    if value { BOOL_TRUE_LITERAL } else { BOOL_FALSE_LITERAL }
}

/// Returns `true` when the encoded value lies inside the reserved sentinel range.
#[inline]
pub const fn is_reserved_sentinel(value: i64) -> bool {
    value >= NIL_VALUE && value <= BOOL_TRUE_VALUE
}

/// Convert a high-level `Val` into its raw encoded form. Currently supports
/// immediates (`Int`, `Bool`, `Nil`). Higher-level handles (lists, maps, etc.)
/// will be wired in once the runtime helper table is implemented.
pub fn encode_immediate(val: &Val) -> Result<i64> {
    match val {
        Val::Nil => Ok(NIL_VALUE),
        Val::Bool(flag) => Ok(if *flag { BOOL_TRUE_VALUE } else { BOOL_FALSE_VALUE }),
        Val::Int(int) => {
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
            other.type_name()
        )),
    }
}

/// Convert a raw encoded value produced by the LLVM backend back into a `Val`
/// representing the corresponding immediate literal.
#[inline]
#[allow(dead_code)] // will be used by upcoming runtime glue translating back into Val
pub fn decode_immediate(value: i64) -> Val {
    if value == NIL_VALUE {
        Val::Nil
    } else if value == BOOL_FALSE_VALUE {
        Val::Bool(false)
    } else if value == BOOL_TRUE_VALUE {
        Val::Bool(true)
    } else {
        Val::Int(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodes_nil_and_bools() {
        assert_eq!(encode_immediate(&Val::Nil).unwrap(), NIL_VALUE);
        assert_eq!(encode_immediate(&Val::Bool(false)).unwrap(), BOOL_FALSE_VALUE);
        assert_eq!(encode_immediate(&Val::Bool(true)).unwrap(), BOOL_TRUE_VALUE);
    }

    #[test]
    fn rejects_reserved_integers() {
        for reserved in [NIL_VALUE, BOOL_FALSE_VALUE, BOOL_TRUE_VALUE] {
            let err = encode_immediate(&Val::Int(reserved)).unwrap_err();
            assert!(err.to_string().contains("conflicts with reserved sentinel"));
        }
    }

    #[test]
    fn encodes_regular_integers() {
        assert_eq!(encode_immediate(&Val::Int(0)).unwrap(), 0);
        assert_eq!(encode_immediate(&Val::Int(123)).unwrap(), 123);
    }

    #[test]
    fn decodes_immediates() {
        assert!(matches!(decode_immediate(NIL_VALUE), Val::Nil));
        assert!(matches!(decode_immediate(BOOL_FALSE_VALUE), Val::Bool(false)));
        assert!(matches!(decode_immediate(BOOL_TRUE_VALUE), Val::Bool(true)));
        assert!(matches!(decode_immediate(42), Val::Int(42)));
    }
}
