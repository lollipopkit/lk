//! Native `regex`: the same `regex` crate the stdlib module uses, with the same
//! bounded compile cache.
//!
//! Sharing the crate is what makes the syntax, the match semantics *and* the
//! parse-error text one rule — `regex.is_match("(", x)` raises a three-line
//! message that is the crate's own `Display`, and a caught error's message is
//! program output.
//!
//! The cache is not an optimisation detail either: compiling a pattern costs
//! far more than matching with it, so a pattern inside a loop is the normal
//! case. The stdlib module caches up to 128 patterns and clears wholesale when
//! it fills; this mirrors that, including the limit, so the two back ends have
//! the same worst case rather than one of them quietly growing without bound.
//!
//! Locking discipline (see `chan.rs`): a raise `longjmp`s past Rust drops, so
//! the compile error is raised **after** the guard is gone, never while holding
//! it.

use alloc::string::{String, ToString as _};
use alloc::vec::Vec;
use core::ffi::{CStr, c_char, c_void};

use regex::Regex;

/// The stdlib module's limit, for the same reason: an unbounded cache keyed by
/// a program-supplied string is a leak with extra steps.
const CACHE_LIMIT: usize = 128;

fn cache() -> &'static std::sync::Mutex<hashbrown::HashMap<String, Regex>> {
    static CACHE: std::sync::OnceLock<std::sync::Mutex<hashbrown::HashMap<String, Regex>>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| std::sync::Mutex::new(hashbrown::HashMap::new()))
}

fn view(text: *const c_char) -> &'static str {
    if text.is_null() {
        return "";
    }
    // SAFETY: LK strings reaching the ABI are NUL-terminated and outlive the call.
    unsafe { CStr::from_ptr(text) }.to_str().unwrap_or("")
}

/// Compiles or reuses a pattern. Returns the error text rather than raising, so
/// the caller can raise with no lock guard alive.
fn compiled(pattern: &str) -> Result<Regex, String> {
    if let Ok(cache) = cache().lock()
        && let Some(regex) = cache.get(pattern)
    {
        return Ok(regex.clone());
    }
    let regex = Regex::new(pattern).map_err(|err| alloc::format!("invalid regex: {err}"))?;
    if let Ok(mut cache) = cache().lock() {
        if cache.len() >= CACHE_LIMIT {
            cache.clear();
        }
        cache.insert(pattern.to_string(), regex.clone());
    }
    Ok(regex)
}

fn regex_or_raise(pattern: *const c_char) -> Regex {
    match compiled(view(pattern)) {
        Ok(regex) => regex,
        Err(message) => crate::panic::raise_str(&message),
    }
}

fn str_list(values: Vec<String>) -> *mut c_void {
    let mut list: Vec<*const c_char> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = crate::lkstr::arena_c_string(alloc::ffi::CString::new(value).unwrap_or_default());
        list.push(ptr.cast_const());
    }
    crate::state::arena_handle(list)
}

/// One match, as the stdlib module's `match_map` builds it: keys `text`,
/// `start`, `end`, in that insertion order and through the VM's own two-stage
/// map construction, because the iteration order is what gets printed.
fn match_map(text: &str, start: usize, end: usize) -> *mut c_void {
    let pairs = alloc::vec![
        (
            String::from("text"),
            crate::lkdyn::lkrt_dyn_from_str(
                crate::lkstr::arena_c_string(alloc::ffi::CString::new(text).unwrap_or_default()).cast_const()
            )
        ),
        (String::from("start"), crate::lkdyn::lkrt_dyn_from_i64(start as i64)),
        (String::from("end"), crate::lkdyn::lkrt_dyn_from_i64(end as i64)),
    ];
    crate::vm_mirror::str_dyn_map_mirrored(pairs)
}

/// `regex.find(text, pattern)` — the match map, or nil when there is none.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_regex_find(text: *const c_char, pattern: *const c_char) -> crate::lkdyn::LkDyn {
    let regex = regex_or_raise(pattern);
    match regex.find(view(text)) {
        Some(m) => crate::lkdyn::lkrt_dyn_from_map(match_map(m.as_str(), m.start(), m.end())),
        None => crate::lkdyn::lkrt_dyn_from_nil(),
    }
}

/// `regex.find_all(text, pattern)` — every match, as a list of match maps.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_regex_find_all(text: *const c_char, pattern: *const c_char) -> *mut c_void {
    let regex = regex_or_raise(pattern);
    let values: Vec<crate::lkdyn::LkDyn> = regex
        .find_iter(view(text))
        .map(|m| crate::lkdyn::lkrt_dyn_from_map(match_map(m.as_str(), m.start(), m.end())))
        .collect();
    crate::state::arena_handle(values)
}

/// `regex.captures(text, pattern)` — group 0 first, then each group, with
/// **nil for a group that did not participate**; nil when nothing matched at
/// all.
///
/// Two different nils, and they are not interchangeable: `captures("abc", "z")`
/// is nil because there was no match, while `captures("xaq", "(a)(z)?")` is
/// `["a", "a", nil]` — a list whose last element is nil.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_regex_captures(text: *const c_char, pattern: *const c_char) -> crate::lkdyn::LkDyn {
    let regex = regex_or_raise(pattern);
    let Some(captures) = regex.captures(view(text)) else {
        return crate::lkdyn::lkrt_dyn_from_nil();
    };
    let values: Vec<crate::lkdyn::LkDyn> = captures
        .iter()
        .map(|capture| match capture {
            Some(value) => crate::lkdyn::lkrt_dyn_from_str(
                crate::lkstr::arena_c_string(alloc::ffi::CString::new(value.as_str()).unwrap_or_default()).cast_const(),
            ),
            None => crate::lkdyn::lkrt_dyn_from_nil(),
        })
        .collect();
    crate::lkdyn::lkrt_dyn_from_list(crate::state::arena_handle(values))
}

/// `regex.is_match(text, pattern)`.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_regex_is_match(text: *const c_char, pattern: *const c_char) -> i64 {
    let regex = regex_or_raise(pattern);
    i64::from(regex.is_match(view(text)))
}

/// `regex.split(text, pattern)` — a string list, empty pieces included, exactly
/// as `Regex::split` yields them.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_regex_split(text: *const c_char, pattern: *const c_char) -> *mut c_void {
    let regex = regex_or_raise(pattern);
    str_list(regex.split(view(text)).map(ToString::to_string).collect())
}

/// `regex.replace(text, pattern, replacement)` — every match, and the
/// replacement keeps the crate's `$1` capture syntax.
#[unsafe(no_mangle)]
pub extern "C" fn lkrt_regex_replace(
    text: *const c_char,
    pattern: *const c_char,
    replacement: *const c_char,
) -> *mut c_char {
    let regex = regex_or_raise(pattern);
    let replaced = regex.replace_all(view(text), view(replacement)).into_owned();
    crate::lkstr::arena_c_string(alloc::ffi::CString::new(replaced).unwrap_or_default())
}
