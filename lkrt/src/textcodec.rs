//! Native `encoding.base64` / `encoding.hex` / `encoding.url`, mirroring the
//! stdlib module's exact conventions.
//!
//! Same argument as [`crate::encoding`] and `datetime`: the *same crates* the
//! stdlib module uses (`base64`, `hex`), so the produced text is byte-identical
//! and the differential corpora can compare stdout directly. Where the stdlib
//! writes the algorithm out by hand — percent-encoding a URI component — this
//! writes the same one, because the pair's two directions have to agree with
//! each other before they agree with anything else.
//!
//! `base64.decode` and `hex.decode` answer `Bytes`, which is an arena handle
//! ([`crate::lkbytes`]) — the same shape a `List` has. Both raise on malformed
//! input with the stdlib module's exact message, because the raise text is part
//! of the contract.

use alloc::ffi::CString;
use alloc::string::String;
use core::ffi::{CStr, c_char};

use base64::Engine as _;

use crate::lkbytes::bytes_handle;
use crate::lkstr::arena_c_string;

fn view<'a>(p: *const c_char) -> &'a str {
    if p.is_null() {
        return "";
    }
    // SAFETY: non-null pointers are NUL-terminated per the ABI.
    unsafe { CStr::from_ptr(p) }.to_str().unwrap_or("")
}

fn out(text: String) -> *mut c_char {
    arena_c_string(CString::new(text).unwrap_or_default())
}

/// `encoding.base64.encode(data)` — standard alphabet with padding, the
/// stdlib module's `STANDARD` engine.
///
/// # Safety
/// `data` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_base64_encode(data: *const c_char) -> *mut c_char {
    out(base64::engine::general_purpose::STANDARD.encode(view(data).as_bytes()))
}

/// `encoding.hex.encode(data)` — lowercase, the `hex` crate's `encode`.
///
/// # Safety
/// `data` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_hex_encode(data: *const c_char) -> *mut c_char {
    out(hex::encode(view(data).as_bytes()))
}

/// `encoding.base64.decode(text)` — raises on malformed input.
///
/// # Safety
/// `text` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_base64_decode(text: *const c_char) -> *mut core::ffi::c_void {
    match base64::engine::general_purpose::STANDARD.decode(view(text).as_bytes()) {
        Ok(bytes) => bytes_handle(bytes),
        Err(error) => crate::panic::raise_str(&alloc::format!("invalid base64 data: {error}")),
    }
}

/// `encoding.hex.decode(text)` — raises on malformed input.
///
/// # Safety
/// `text` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_hex_decode(text: *const c_char) -> *mut core::ffi::c_void {
    match hex::decode(view(text)) {
        Ok(bytes) => bytes_handle(bytes),
        Err(error) => crate::panic::raise_str(&alloc::format!("invalid hex data: {error}")),
    }
}

/// Whether `byte` survives a URI component unescaped.
///
/// `encodeURIComponent`'s set, which is what the stdlib module uses:
/// `A-Za-z0-9-_.!~*'()`.
fn unreserved(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'!' | b'~' | b'*' | b'\'' | b'(' | b')')
}

/// `encoding.url.encode_component(value)`.
///
/// # Safety
/// `value` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_url_encode_component(value: *const c_char) -> *mut c_char {
    let value = view(value);
    let mut encoded = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        if unreserved(byte) {
            encoded.push(byte as char);
        } else {
            encoded.push('%');
            encoded.push_str(&alloc::format!("{byte:02X}"));
        }
    }
    out(encoded)
}

/// `encoding.url.decode_component(value)` — raises on a malformed escape, with
/// the stdlib module's exact three messages.
///
/// # Safety
/// `value` must be a valid C string, or null.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn lkrt_url_decode_component(value: *const c_char) -> *mut c_char {
    let bytes = view(value).as_bytes();
    let mut decoded = alloc::vec::Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let Some(escape) = bytes.get(index + 1..index + 3) else {
            crate::panic::raise_str("invalid percent encoding: incomplete escape");
        };
        let Ok(escape) = core::str::from_utf8(escape) else {
            crate::panic::raise_str("invalid percent encoding: non-UTF-8 escape");
        };
        let Ok(byte) = u8::from_str_radix(escape, 16) else {
            crate::panic::raise_str("invalid percent encoding: expected two hex digits");
        };
        decoded.push(byte);
        index += 3;
    }
    match String::from_utf8(decoded) {
        Ok(text) => out(text),
        Err(error) => crate::panic::raise_str(&alloc::format!("invalid percent-encoded UTF-8: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(input: &str) -> String {
        let c = CString::new(input).expect("no interior NUL");
        let ptr = unsafe { lkrt_url_encode_component(c.as_ptr()) };
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }

    fn decoded(input: &str) -> String {
        let c = CString::new(input).expect("no interior NUL");
        let ptr = unsafe { lkrt_url_decode_component(c.as_ptr()) };
        unsafe { CStr::from_ptr(ptr) }.to_string_lossy().into_owned()
    }

    /// The pair's two directions have to agree with each other. They did not:
    /// the encoder was *form* encoding (a space became `+`) while the decoder
    /// only undid `%XX`.
    #[test]
    fn a_component_round_trips() {
        for original in ["a b&c=d", "", "plain", "+literal+", "100%", "héllo", "a/b?c#d"] {
            assert_eq!(encoded(original).as_str(), encoded(original).as_str());
            assert_eq!(decoded(&encoded(original)), original, "round trip of {original:?}");
        }
        // A space is `%20`, not `+`: this is a component, and `+` is the literal
        // `+` there (`encodeURIComponent`'s rule).
        assert_eq!(encoded("a b"), "a%20b");
        assert_eq!(decoded("a+b"), "a+b");
    }

    #[test]
    fn base64_and_hex_match_their_crates() {
        let hi = CString::new("hi").expect("no interior NUL");
        let b64 = unsafe { lkrt_base64_encode(hi.as_ptr()) };
        assert_eq!(unsafe { CStr::from_ptr(b64) }.to_str().expect("utf-8"), "aGk=");
        let hex = unsafe { lkrt_hex_encode(hi.as_ptr()) };
        assert_eq!(unsafe { CStr::from_ptr(hex) }.to_str().expect("utf-8"), "6869");
    }
}
