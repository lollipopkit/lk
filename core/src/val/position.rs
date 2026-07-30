//! What a position means against a container of a given length.
//!
//! One rule, one place. It was written inside the VM's method dispatch as
//! `pub(super)` helpers, which put it out of reach of the **stdlib module
//! crates** — and `bytes` is implemented there. So `b.slice(1, -1)` (the method,
//! through the VM's dispatch) answered `Bytes([98,99,100])` while
//! `bytes.slice(b, 1, -1)` (the module, through its own `usize_arg`) raised
//! "expects a non-negative integer": the same operation, two spellings, two
//! answers. `bytes.get` likewise.
//!
//! `docs/semantics.md` had already ruled that a negative position counts from
//! the end everywhere, and the comment on the read helper even said "List and
//! Bytes raised" in the past tense — a ruling whose fourth site never received
//! it, because it could not see the code that implemented it.
//!
//! The native side keeps its own mirror (`lkrt::lkslice::resolve_position`),
//! which is the documented pattern: lkrt must not depend on the front end.

#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

use crate::val::RuntimeVal;
use anyhow::{Result, bail};

/// A *read* position: negative counts from the end, and the result is clamped
/// into `0..=len`.
///
/// Clamping rather than raising is the read side's rule throughout the language:
/// reading past the end is nil (or an empty window), which is a meaning a program
/// can have.
pub fn read_position(value: &RuntimeVal, len: usize, context: &str) -> Result<usize> {
    let RuntimeVal::Int(index) = value else {
        bail!("{context} must be Int");
    };
    let len = len as i64;
    let resolved = if *index < 0 { len + *index } else { *index };
    Ok(resolved.clamp(0, len) as usize)
}

/// A read position that may miss: negative counts from the end, and out of range
/// is `None` rather than a clamp.
///
/// `xs[9]` and `bytes.get(b, 9)` are nil, not the last element — clamping would
/// invent an answer. The distinction from [`read_position`] is exactly that a
/// *window* has a meaningful clamp and an *element* does not.
pub fn element_position(value: &RuntimeVal, len: usize, context: &str) -> Result<Option<usize>> {
    let RuntimeVal::Int(index) = value else {
        bail!("{context} must be Int");
    };
    let resolved = if *index < 0 { len as i64 + *index } else { *index };
    if resolved < 0 || resolved >= len as i64 {
        return Ok(None);
    }
    Ok(Some(resolved as usize))
}

/// A *write* position: negative counts from the end, and still out of range is an
/// error.
///
/// Reading past the end is nil; writing past it is not something a program can
/// mean. The caller does the upper-bound check, because `insert` accepts `len`
/// and the others do not.
pub fn write_position(value: &RuntimeVal, len: usize, context: &str) -> Result<usize> {
    let RuntimeVal::Int(index) = value else {
        bail!("{context} must be Int");
    };
    let resolved = if *index < 0 { len as i64 + *index } else { *index };
    if resolved < 0 {
        bail!("{context} {index} is before the start of a list of {len}");
    }
    Ok(resolved as usize)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_negative_position_counts_from_the_end() {
        let five = 5;
        assert_eq!(read_position(&RuntimeVal::Int(-1), five, "ctx").expect("ok"), 4);
        assert_eq!(element_position(&RuntimeVal::Int(-1), five, "ctx").expect("ok"), Some(4));
        assert_eq!(write_position(&RuntimeVal::Int(-1), five, "ctx").expect("ok"), 4);
    }

    /// A window clamps, an element misses. Both are read-side rules, and the
    /// difference is whether there is an answer to invent.
    #[test]
    fn out_of_range_clamps_for_a_window_and_misses_for_an_element() {
        assert_eq!(read_position(&RuntimeVal::Int(99), 5, "ctx").expect("ok"), 5);
        assert_eq!(read_position(&RuntimeVal::Int(-99), 5, "ctx").expect("ok"), 0);
        assert_eq!(element_position(&RuntimeVal::Int(99), 5, "ctx").expect("ok"), None);
        assert_eq!(element_position(&RuntimeVal::Int(-99), 5, "ctx").expect("ok"), None);
        // A write that is still before the start is an error, not a clamp.
        assert!(write_position(&RuntimeVal::Int(-99), 5, "ctx").is_err());
    }

    #[test]
    fn a_non_integer_position_is_refused_by_every_rule() {
        let text = RuntimeVal::Bool(true);
        assert!(read_position(&text, 5, "ctx").is_err());
        assert!(element_position(&text, 5, "ctx").is_err());
        assert!(write_position(&text, 5, "ctx").is_err());
    }
}
