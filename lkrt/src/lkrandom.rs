//! Native `random`: the same `rand` crate the stdlib module uses, and the same
//! bounds, defaults and refusals.
//!
//! Values cannot be compared between the back ends — that is what makes them
//! random — so what has to match is everything *around* the value: an inclusive
//! range, `0.5` as the default probability, the 16 MiB byte cap, nil for an
//! empty `choice`, and each refusal's exact sentence, since a caught error's
//! message is program output.
//!
//! None of these entries may be `Pure` in the ABI schema. CSE merges equal
//! `Pure` calls in a dominance scope, and `random.int(1, 6)` twice is two
//! rolls; `nondeterministic_entries_are_not_pure` pins that.
//!
//! `std`-only: the entropy source is the OS's.

use alloc::vec::Vec;
use core::ffi::c_void;

use rand::Rng as _;

/// The stdlib module's cap, and its wording depends on the number.
const MAX_RANDOM_BYTES: usize = 16 * 1024 * 1024;

/// `random.int(min, max)` — **inclusive** at both ends.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_random_int(min: i64, max: i64) -> i64 {
    if max < min {
        crate::panic::raise_str("random.int() max must be >= min");
    }
    rand::rng().random_range(min..=max)
}

/// `random.float()` — the half-open unit interval, `rand`'s `random::<f64>()`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_random_float() -> f64 {
    rand::rng().random()
}

/// `random.bool()` — a fair coin, the module's default probability.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_random_bool() -> i64 {
    i64::from(rand::rng().random_bool(0.5))
}

/// `random.bool(probability)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_random_bool_p(probability: f64) -> i64 {
    if !(0.0..=1.0).contains(&probability) {
        crate::panic::raise_str("random.bool() probability must be in 0..=1");
    }
    i64::from(rand::rng().random_bool(probability))
}

/// `random.bytes(len)`.
///
/// Two different refusals, and the negative one is not this module's sentence:
/// the stdlib reads the length through its shared `usize_arg`, so a negative
/// length reports as a *type* problem (`random.bytes len expects non-negative
/// Int, got Int`) while an oversized one reports the cap.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_random_bytes(len: i64) -> *mut c_void {
    if len < 0 {
        crate::panic::raise_str("random.bytes len expects non-negative Int, got Int");
    }
    let len = len as usize;
    if len > MAX_RANDOM_BYTES {
        crate::panic::raise_str(&alloc::format!("random.bytes() len exceeds {MAX_RANDOM_BYTES}"));
    }
    let mut data = alloc::vec![0u8; len];
    rand::rng().fill(data.as_mut_slice());
    crate::lkbytes::bytes_handle(data)
}

/// A uniform index into a list of `len` elements, or `None` when it is empty
/// (`random.choice([])` is nil, not a raise).
fn choice_index(len: usize) -> Option<usize> {
    (len > 0).then(|| rand::rng().random_range(0..len))
}

/// The module's shuffle: Fisher–Yates walking down from the end, the same
/// direction and the same `random_range(0..=i)` the stdlib uses.
fn shuffle_in_place<T>(values: &mut [T]) {
    for i in (1..values.len()).rev() {
        let j = rand::rng().random_range(0..=i);
        values.swap(i, j);
    }
}

macro_rules! choice_and_shuffle {
    ($choice:ident, $shuffle:ident, $elem:ty, $list:ty, $to_dyn:expr) => {
        /// `random.choice(xs)` for one list carrier.
        ///
        /// # Safety
        /// `handle` must be a live list handle of this carrier.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $choice(handle: *mut c_void) -> crate::lkdyn::LkDyn {
            // SAFETY: a live list handle of this carrier, as the ABI declares.
            let values: &$list = unsafe { &*(handle as *mut $list) };
            match choice_index(values.len()) {
                Some(index) => {
                    let value: $elem = values[index].clone();
                    #[allow(clippy::redundant_closure_call)]
                    ($to_dyn)(value)
                }
                None => crate::lkdyn::lkrt_dyn_from_nil(),
            }
        }

        /// `random.shuffle(xs)` for one list carrier — a new list, like the VM's
        /// (the module builds a fresh one rather than reordering the argument).
        ///
        /// # Safety
        /// `handle` must be a live list handle of this carrier.
        #[unsafe(no_mangle)]
        pub unsafe extern "C" fn $shuffle(handle: *mut c_void) -> *mut c_void {
            // SAFETY: a live list handle of this carrier, as the ABI declares.
            let values: &$list = unsafe { &*(handle as *mut $list) };
            let mut out: $list = values.clone();
            shuffle_in_place(out.as_mut_slice());
            crate::state::arena_handle(out)
        }
    };
}

choice_and_shuffle!(
    lkrt_random_choice_i64,
    lkrt_random_shuffle_i64,
    i64,
    Vec<i64>,
    crate::lkdyn::lkrt_dyn_from_i64
);
choice_and_shuffle!(
    lkrt_random_choice_f64,
    lkrt_random_shuffle_f64,
    f64,
    Vec<f64>,
    crate::lkdyn::lkrt_dyn_from_f64
);
choice_and_shuffle!(
    lkrt_random_choice_str,
    lkrt_random_shuffle_str,
    *const core::ffi::c_char,
    Vec<*const core::ffi::c_char>,
    crate::lkdyn::lkrt_dyn_from_str
);
choice_and_shuffle!(
    lkrt_random_choice_dyn,
    lkrt_random_shuffle_dyn,
    crate::lkdyn::LkDyn,
    Vec<crate::lkdyn::LkDyn>,
    |value| value
);
