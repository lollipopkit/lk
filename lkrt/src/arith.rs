//! Divisor-guarded integer/float arithmetic.
//!
//! The VM treats division or remainder by zero as a fatal runtime error
//! (`bail!("DivInt divisor is zero")` / `"ModInt divisor is zero"`, see
//! `core/src/vm/exec/arithmetic.rs`). Native AOT previously emitted raw LLVM
//! `sdiv`/`fdiv`/`frem`, where a zero divisor (and the `i64::MIN / -1` overflow)
//! is undefined behaviour. These helpers make the divisor-zero case a
//! deterministic `abort()` — matching how AOT already lowers `panic` — and use
//! wrapping integer division so the `MIN / -1` overflow is defined rather than UB.
//!
//! Keeping the guard here (rather than inline in codegen) is the divisor-zero
//! item from the AOT redesign: a single audited place where VM and AOT agree.

/// `lhs / rhs` for integers, aborting on a zero divisor. `i64::MIN / -1` wraps to
/// `i64::MIN` instead of overflowing (defined, matching release-mode wrapping).
// `alloc`, not the std prelude: this module is part of the computation-only
// subset that builds without an OS.
#[allow(unused_imports)]
use alloc::{
    boxed::Box,
    format,
    string::{String, ToString},
    vec,
    vec::Vec,
};

#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_div_checked(lhs: i64, rhs: i64) -> i64 {
    if rhs == 0 {
        crate::panic::raise_str("division by zero");
    }
    lhs.wrapping_div(rhs)
}

/// `lhs << rhs`, raising when the shift amount is not in `0..=63`.
///
/// The message is the VM's, word for word — including the offending amount.
/// Cross-backend error text is not guaranteed identical in general (see
/// `docs/semantics.md`), but a *catchable* arithmetic failure is one a program
/// can branch on, so these few are aligned by hand. `%` by zero was not: the VM
/// said `ModInt divisor is zero` and this side said `Division by zero` — two
/// different strings, both wrong about which operator failed.
///
/// The hardware would mask the amount to 63 and produce a number; that number
/// is not what the program asked for.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_shl_checked(lhs: i64, rhs: i64) -> i64 {
    if !(0..64).contains(&rhs) {
        crate::panic::raise_str(&format!("shift amount {rhs} is out of range 0..63"));
    }
    lhs.wrapping_shl(rhs as u32)
}

/// `lhs >> rhs`, arithmetic (the sign bit is replicated), same range rule.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_shr_checked(lhs: i64, rhs: i64) -> i64 {
    if !(0..64).contains(&rhs) {
        crate::panic::raise_str(&format!("shift amount {rhs} is out of range 0..63"));
    }
    lhs.wrapping_shr(rhs as u32)
}

/// `lhs >> rhs`, *logical* — zeros come in at the top — with the same range rule.
///
/// The one shift `>>` cannot always be. Every value in this language rides an
/// `i64` carrier, so for a `u8`, `u16` or `u32` the high bits are zero and an
/// arithmetic shift happens to give the right answer. A `u64` fills the carrier:
/// bit 63 *is* the sign bit, and shifting `1u64 << 63` right by 63 answered -1
/// instead of 1 — silently, on both backends, which is what a physical address
/// or a page-table entry is made of.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_u64_shr_checked(lhs: i64, rhs: i64) -> i64 {
    if !(0..64).contains(&rhs) {
        crate::panic::raise_str(&format!("shift amount {rhs} is out of range 0..63"));
    }
    ((lhs as u64).wrapping_shr(rhs as u32)) as i64
}

/// `lhs < rhs`, unsigned. Answers 1 or 0.
///
/// The one comparison a `u64` cannot borrow from `Int`. Every value rides an
/// `i64` carrier, so a `u64` with bit 63 set *is* a negative carrier and a
/// signed compare puts it below 1. One primitive rather than four: `a > b` is
/// `b < a`, and the two inclusive forms are those negated.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_u64_lt(lhs: i64, rhs: i64) -> i64 {
    i64::from((lhs as u64) < (rhs as u64))
}

/// `lhs / rhs`, unsigned, aborting on a zero divisor.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_u64_div(lhs: i64, rhs: i64) -> i64 {
    if rhs == 0 {
        crate::panic::raise_str("division by zero");
    }
    ((lhs as u64) / (rhs as u64)) as i64
}

/// `lhs % rhs`, unsigned, aborting on a zero divisor.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_u64_rem(lhs: i64, rhs: i64) -> i64 {
    if rhs == 0 {
        crate::panic::raise_str("modulo by zero");
    }
    ((lhs as u64) % (rhs as u64)) as i64
}

/// `value as Float`, reading the carrier as unsigned.
///
/// The last place a `u64` is read as an `i64`. A value with bit 63 set is a
/// negative carrier, so the ordinary conversion answers a negative float — and
/// unlike a comparison or a divide, nothing about the result *looks* wrong until
/// it is compared with zero.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_u64_to_f64(value: i64) -> f64 {
    (value as u64) as f64
}

/// `lhs % rhs` for integers, aborting on a zero divisor. `i64::MIN % -1` wraps to
/// `0` instead of overflowing.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_mod_checked(lhs: i64, rhs: i64) -> i64 {
    if rhs == 0 {
        crate::panic::raise_str("modulo by zero");
    }
    lhs.wrapping_rem(rhs)
}

/// `lhs / rhs` for floats — IEEE, so a zero divisor gives an infinity or a
/// NaN rather than raising.
///
/// The name keeps `_checked` because it is the ABI symbol both backends were
/// built against; there is nothing left to check. It used to raise, to match a
/// VM that raised — and both were wrong about `Float`, which *is* `f64`.
// TODO: rename to `lkrt_f64_div` once an ABI version bump is due anyway.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_f64_div_checked(lhs: f64, rhs: f64) -> f64 {
    lhs / rhs
}

/// `lhs % rhs` for floats — IEEE, so a zero divisor gives a NaN.
// TODO: rename to `lkrt_f64_mod` alongside `lkrt_f64_div_checked`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_f64_mod_checked(lhs: f64, rhs: f64) -> f64 {
    lhs % rhs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn integer_division_and_remainder() {
        assert_eq!(lkrt_i64_div_checked(7, 2), 3);
        assert_eq!(lkrt_i64_mod_checked(7, 2), 1);
        assert_eq!(lkrt_i64_div_checked(-7, 2), -3);
        // MIN / -1 must not overflow (UB in raw sdiv); wrapping gives MIN / 0.
        assert_eq!(lkrt_i64_div_checked(i64::MIN, -1), i64::MIN);
        assert_eq!(lkrt_i64_mod_checked(i64::MIN, -1), 0);
    }

    #[test]
    fn float_division_and_remainder() {
        assert_eq!(lkrt_f64_div_checked(7.0, 2.0), 3.5);
        assert_eq!(lkrt_f64_mod_checked(7.0, 2.0), 1.0);
    }
}
