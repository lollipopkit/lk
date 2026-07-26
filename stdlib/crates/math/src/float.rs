//! Float methods for no_std.
//!
//! `f64`'s inherent maths methods (`sqrt`, `sin`, `ln`, …) are defined in std,
//! not core — under no_std they simply do not exist. This extension trait
//! restores them by name so every call site reads identically in both builds,
//! with `libm` (the same routines Rust's own std uses on many targets)
//! providing the implementations.
//!
//! Compiled only under no_std: with std present the inherent methods win and
//! this module would be dead weight.

#[allow(dead_code)]
pub trait FloatExt {
    fn sqrt(self) -> f64;
    fn cbrt(self) -> f64;
    fn hypot(self, other: f64) -> f64;
    fn powf(self, exponent: f64) -> f64;
    fn exp(self) -> f64;
    fn ln(self) -> f64;
    fn log10(self) -> f64;
    fn log2(self) -> f64;
    fn sin(self) -> f64;
    fn cos(self) -> f64;
    fn tan(self) -> f64;
    fn asin(self) -> f64;
    fn acos(self) -> f64;
    fn atan(self) -> f64;
    fn atan2(self, other: f64) -> f64;
    fn sinh(self) -> f64;
    fn cosh(self) -> f64;
    fn tanh(self) -> f64;
    fn floor(self) -> f64;
    fn ceil(self) -> f64;
    fn round(self) -> f64;
    fn trunc(self) -> f64;
    fn fract(self) -> f64;
}

impl FloatExt for f64 {
    fn sqrt(self) -> f64 {
        libm::sqrt(self)
    }
    fn cbrt(self) -> f64 {
        libm::cbrt(self)
    }
    fn hypot(self, other: f64) -> f64 {
        libm::hypot(self, other)
    }
    fn powf(self, exponent: f64) -> f64 {
        libm::pow(self, exponent)
    }
    fn exp(self) -> f64 {
        libm::exp(self)
    }
    fn ln(self) -> f64 {
        libm::log(self)
    }
    fn log10(self) -> f64 {
        libm::log10(self)
    }
    fn log2(self) -> f64 {
        libm::log2(self)
    }
    fn sin(self) -> f64 {
        libm::sin(self)
    }
    fn cos(self) -> f64 {
        libm::cos(self)
    }
    fn tan(self) -> f64 {
        libm::tan(self)
    }
    fn asin(self) -> f64 {
        libm::asin(self)
    }
    fn acos(self) -> f64 {
        libm::acos(self)
    }
    fn atan(self) -> f64 {
        libm::atan(self)
    }
    fn atan2(self, other: f64) -> f64 {
        libm::atan2(self, other)
    }
    fn sinh(self) -> f64 {
        libm::sinh(self)
    }
    fn cosh(self) -> f64 {
        libm::cosh(self)
    }
    fn tanh(self) -> f64 {
        libm::tanh(self)
    }
    fn floor(self) -> f64 {
        libm::floor(self)
    }
    fn ceil(self) -> f64 {
        libm::ceil(self)
    }
    fn round(self) -> f64 {
        libm::round(self)
    }
    fn trunc(self) -> f64 {
        libm::trunc(self)
    }
    /// std defines this as `self - self.trunc()`; libm has no direct
    /// equivalent, and `modf` returns the pair in the other order.
    fn fract(self) -> f64 {
        self - libm::trunc(self)
    }
}
