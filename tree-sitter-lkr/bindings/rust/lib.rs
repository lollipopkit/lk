//! Tree-sitter bindings for LKR
//!
//! This crate provides Rust bindings for the tree-sitter LKR grammar.
//! Use `language()` to obtain the tree-sitter Language for LKR parsing.

use std::sync::OnceLock;

#[cfg(not(feature = "link"))]
mod scanner {
    pub extern "C" fn tree_sitter_lkr_external_scanner_create() -> *mut std::ffi::c_void {
        std::ptr::null_mut()
    }
    pub extern "C" fn tree_sitter_lkr_external_scanner_destroy(_: *mut std::ffi::c_void) {}
    pub extern "C" fn tree_sitter_lkr_external_scanner_scan(
        _: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
        _: *const bool,
    ) -> bool {
        false
    }
    pub extern "C" fn tree_sitter_lkr_external_scanner_serialize(
        _: *mut std::ffi::c_void,
        _: *mut std::ffi::c_char,
    ) -> u32 {
        0
    }
    pub extern "C" fn tree_sitter_lkr_external_scanner_deserialize(
        _: *mut std::ffi::c_void,
        _: *const std::ffi::c_char,
        _: u32,
    ) {
    }
}

static LANGUAGE: OnceLock<tree_sitter::Language> = OnceLock::new();

/// Get the tree-sitter Language for LKR.
pub fn language() -> tree_sitter::Language {
    LANGUAGE.get_or_init(|| unsafe { tree_sitter_lkr() }).clone()
}

extern "C" {
    fn tree_sitter_lkr() -> tree_sitter::Language;
}

/// Get the NODE_TYPES map for LKR grammar.
pub const NODE_TYPES: &str = include_str!("../../src/node-types.json");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language() {
        let lang = language();
        assert!(lang.node_kind_count() > 0);
    }

    #[test]
    fn test_node_types() {
        assert!(!NODE_TYPES.is_empty());
    }
}