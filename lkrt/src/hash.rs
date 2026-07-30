//! Native `hash`: the same crates the stdlib module uses, so a digest is
//! byte-identical to the VM's.
//!
//! `sha2`/`sha1`/`crc32fast` are shared rather than reimplemented — the rule
//! that keeps base64/hex text and datetime formatting identical across the two
//! back ends. The hex rendering matches too, because both sides spell it
//! `format!("{:x}", digest)` and that is the crates' own `LowerHex`.
//!
//! `fnv64` is the exception: FNV-1a is four lines with two constants and no
//! crate in the graph provides it, so the loop genuinely exists twice. What
//! keeps the pair from drifting is `hash_members_answer_the_same_on_both_ends`
//! in the CLI's `clif_differential_test`, which runs both spellings over the
//! same inputs and compares stdout — a *wrong digest* is precisely the thing a
//! differential can see (unlike a lost lowering, which it cannot).
//!
//! Every member takes `Bytes | String` in the language, which is two native
//! argument types and therefore two entry points each. The digest of a string
//! is the digest of its UTF-8 bytes, which is what the VM does as well
//! (`runtime_bytes_or_string_arg`).

// `alloc`, not the std prelude: hashing needs no OS, so this module is part of
// the computation-only subset.
use alloc::format;
use core::ffi::{CStr, c_char, c_void};

use sha1::Digest as _;

fn str_bytes(text: *const c_char) -> &'static [u8] {
    if text.is_null() {
        return &[];
    }
    // SAFETY: LK strings reaching the ABI are NUL-terminated and live for the
    // duration of the call.
    unsafe { CStr::from_ptr(text) }.to_bytes()
}

fn out(text: alloc::string::String) -> *mut c_char {
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(text).unwrap_or_default())
}

fn sha256_of(data: &[u8]) -> *mut c_char {
    out(format!("{:x}", sha2::Sha256::digest(data)))
}

fn sha1_of(data: &[u8]) -> *mut c_char {
    out(format!("{:x}", sha1::Sha1::digest(data)))
}

fn crc32_of(data: &[u8]) -> i64 {
    crc32fast::hash(data) as i64
}

/// FNV-1a, 64-bit — the stdlib `hash` module's loop, constants included.
///
/// Kept in sync by the differential, not by inspection.
pub(crate) fn fnv64_of(data: &[u8]) -> i64 {
    const OFFSET: u64 = 0xcbf29ce484222325;
    const PRIME: u64 = 0x100000001b3;
    let mut hash = OFFSET;
    for byte in data {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(PRIME);
    }
    hash as i64
}

/// `hash.sha256(text)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_hash_sha256_str(text: *const c_char) -> *mut c_char {
    sha256_of(str_bytes(text))
}

/// `hash.sha1(text)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_hash_sha1_str(text: *const c_char) -> *mut c_char {
    sha1_of(str_bytes(text))
}

/// `hash.crc32(text)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_hash_crc32_str(text: *const c_char) -> i64 {
    crc32_of(str_bytes(text))
}

/// `hash.fnv64(text)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_hash_fnv64_str(text: *const c_char) -> i64 {
    fnv64_of(str_bytes(text))
}

/// `hash.sha256(bytes)`.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_hash_sha256_bytes(handle: *mut c_void) -> *mut c_char {
    sha256_of(crate::lkbytes::bytes_slice(handle))
}

/// `hash.sha1(bytes)`.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_hash_sha1_bytes(handle: *mut c_void) -> *mut c_char {
    sha1_of(crate::lkbytes::bytes_slice(handle))
}

/// `hash.crc32(bytes)`.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_hash_crc32_bytes(handle: *mut c_void) -> i64 {
    crc32_of(crate::lkbytes::bytes_slice(handle))
}

/// `hash.fnv64(bytes)`.
///
/// # Safety
/// `handle` must be a live `Bytes` handle.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_hash_fnv64_bytes(handle: *mut c_void) -> i64 {
    fnv64_of(crate::lkbytes::bytes_slice(handle))
}
