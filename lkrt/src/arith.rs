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
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_div_checked(lhs: i64, rhs: i64) -> i64 {
    if rhs == 0 {
        std::process::abort();
    }
    lhs.wrapping_div(rhs)
}

/// `lhs % rhs` for integers, aborting on a zero divisor. `i64::MIN % -1` wraps to
/// `0` instead of overflowing.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_i64_mod_checked(lhs: i64, rhs: i64) -> i64 {
    if rhs == 0 {
        std::process::abort();
    }
    lhs.wrapping_rem(rhs)
}

/// `lhs / rhs` for floats, aborting on a zero divisor to match the VM (which
/// errors on float division by zero rather than producing infinity).
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_f64_div_checked(lhs: f64, rhs: f64) -> f64 {
    if rhs == 0.0 {
        std::process::abort();
    }
    lhs / rhs
}

/// `lhs % rhs` for floats, aborting on a zero divisor to match the VM.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_f64_mod_checked(lhs: f64, rhs: f64) -> f64 {
    if rhs == 0.0 {
        std::process::abort();
    }
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
