//! Typed native runtime support for LK LLVM AOT binaries.
//!
//! This crate is intentionally not the LK VM. It may provide low-level typed
//! helpers that LLVM-generated code links against, but it must not depend on the
//! parser, compiler, `ModuleArtifact`, `VmContext`, or the bytecode executor.

mod abi;
#[cfg(test)]
mod abi_conformance_test;
mod arith;
mod host;
mod io;
mod lkdyn;
mod lklist;
mod lkmap;
mod lkset;
mod lkstr;
mod net;
mod panic;
mod state;
mod vm_mirror;

pub use abi::{
    lkrt_abi_check, lkrt_abi_version, lkrt_abort, lkrt_assert, lkrt_assert_msg, lkrt_cleanup, lkrt_error_clear,
    lkrt_last_error, lkrt_panic, lkrt_string_free,
};
pub use arith::{lkrt_f64_div_checked, lkrt_f64_mod_checked, lkrt_i64_div_checked, lkrt_i64_mod_checked};
pub use host::{
    lkrt_datetime_day_of_week, lkrt_datetime_day_of_year, lkrt_datetime_format, lkrt_datetime_is_weekend,
    lkrt_datetime_now, lkrt_datetime_parse, lkrt_env_get, lkrt_env_get_or, lkrt_env_has, lkrt_env_remove, lkrt_env_set,
    lkrt_fs_canonicalize, lkrt_fs_exists, lkrt_fs_metadata_is_dir, lkrt_fs_metadata_is_file, lkrt_fs_metadata_len,
    lkrt_fs_metadata_readonly, lkrt_fs_read, lkrt_fs_read_dir_list, lkrt_fs_read_to_string, lkrt_fs_temp_dir,
    lkrt_fs_write_bytes, lkrt_fs_write_str, lkrt_math_ceil, lkrt_math_cos, lkrt_math_exp, lkrt_math_floor,
    lkrt_math_pow, lkrt_math_round, lkrt_math_sin, lkrt_math_sqrt, lkrt_os_arch, lkrt_os_clock, lkrt_os_epoch,
    lkrt_os_hostname, lkrt_os_name, lkrt_path_temp_dir, lkrt_process_cwd, lkrt_time_now_ms, lkrt_time_sleep_ms,
};
pub use host::{
    lkrt_math_cbrt, lkrt_math_hypot, lkrt_math_is_nan, lkrt_math_sign_f64, lkrt_math_sign_i64, lkrt_path_sep,
};
pub use io::{lkrt_io_std_flush, lkrt_io_std_read_to_string, lkrt_io_std_write};
pub use lkdyn::{
    LkDyn, lkrt_dyn_add, lkrt_dyn_as_bool, lkrt_dyn_as_f64, lkrt_dyn_as_i64, lkrt_dyn_as_list, lkrt_dyn_as_str,
    lkrt_dyn_display, lkrt_dyn_display_quoted, lkrt_dyn_div, lkrt_dyn_eq, lkrt_dyn_field, lkrt_dyn_from_bool,
    lkrt_dyn_from_f64, lkrt_dyn_from_i64, lkrt_dyn_from_list, lkrt_dyn_from_map, lkrt_dyn_from_maybe_bool,
    lkrt_dyn_from_maybe_f64, lkrt_dyn_from_maybe_i64, lkrt_dyn_from_maybe_str, lkrt_dyn_from_nil, lkrt_dyn_from_str,
    lkrt_dyn_ge, lkrt_dyn_gt, lkrt_dyn_index, lkrt_dyn_le, lkrt_dyn_len_of, lkrt_dyn_lt, lkrt_dyn_mod, lkrt_dyn_mul,
    lkrt_dyn_sub, lkrt_dyn_tag, lkrt_dyn_truthy, lkrt_lklist_dyn_at, lkrt_lklist_dyn_chain, lkrt_lklist_dyn_chunk,
    lkrt_lklist_dyn_contains, lkrt_lklist_dyn_display, lkrt_lklist_dyn_enumerate, lkrt_lklist_dyn_eq,
    lkrt_lklist_dyn_filter_fn, lkrt_lklist_dyn_flatten, lkrt_lklist_dyn_len, lkrt_lklist_dyn_map_fn,
    lkrt_lklist_dyn_new, lkrt_lklist_dyn_push, lkrt_lklist_dyn_reduce_fn, lkrt_lklist_dyn_set, lkrt_lklist_dyn_skip,
    lkrt_lklist_dyn_slice_from, lkrt_lklist_dyn_take, lkrt_lklist_dyn_unique, lkrt_lklist_dyn_zip,
    lkrt_lklist_f64_to_dyn, lkrt_lklist_i64_to_dyn, lkrt_lklist_str_to_dyn,
};
pub use lklist::{
    LkMaybeF64, LkMaybeI64, LkMaybeStr, lkrt_lklist_f64_at, lkrt_lklist_f64_contains, lkrt_lklist_f64_display,
    lkrt_lklist_f64_eq, lkrt_lklist_f64_get_pair, lkrt_lklist_f64_len, lkrt_lklist_f64_new, lkrt_lklist_f64_push,
    lkrt_lklist_f64_set, lkrt_lklist_f64_slice_from, lkrt_lklist_i64_at, lkrt_lklist_i64_chain,
    lkrt_lklist_i64_contains, lkrt_lklist_i64_display, lkrt_lklist_i64_eq, lkrt_lklist_i64_f64_eq,
    lkrt_lklist_i64_filter_fn, lkrt_lklist_i64_from_range, lkrt_lklist_i64_get, lkrt_lklist_i64_get_pair,
    lkrt_lklist_i64_len, lkrt_lklist_i64_map_fn, lkrt_lklist_i64_new, lkrt_lklist_i64_push, lkrt_lklist_i64_reduce_fn,
    lkrt_lklist_i64_reverse, lkrt_lklist_i64_set, lkrt_lklist_i64_skip, lkrt_lklist_i64_slice,
    lkrt_lklist_i64_slice_from, lkrt_lklist_i64_slice_method, lkrt_lklist_i64_sort, lkrt_lklist_i64_take,
    lkrt_lklist_i64_unique, lkrt_lklist_str_at, lkrt_lklist_str_contains, lkrt_lklist_str_display, lkrt_lklist_str_eq,
    lkrt_lklist_str_filter_fn, lkrt_lklist_str_get_pair, lkrt_lklist_str_join, lkrt_lklist_str_len,
    lkrt_lklist_str_map_fn, lkrt_lklist_str_new, lkrt_lklist_str_push, lkrt_lklist_str_slice_from,
    lkrt_maybe_f64_unwrap, lkrt_maybe_i64_unwrap, lkrt_maybe_str_unwrap, lkrt_str_split,
};
pub use lkmap::{
    lkrt_lkmap_i64_f64_get_pair, lkrt_lkmap_i64_f64_len, lkrt_lkmap_i64_f64_new, lkrt_lkmap_i64_f64_set,
    lkrt_lkmap_i64_i64_get_pair, lkrt_lkmap_i64_i64_len, lkrt_lkmap_i64_i64_new, lkrt_lkmap_i64_i64_set,
    lkrt_lkmap_str_dyn_get, lkrt_lkmap_str_dyn_has, lkrt_lkmap_str_dyn_len, lkrt_lkmap_str_dyn_new,
    lkrt_lkmap_str_dyn_set, lkrt_lkmap_str_dyn_without, lkrt_lkmap_str_f64_get_pair, lkrt_lkmap_str_f64_len,
    lkrt_lkmap_str_f64_new, lkrt_lkmap_str_f64_set, lkrt_lkmap_str_f64_set_ik, lkrt_lkmap_str_f64_without,
    lkrt_lkmap_str_i64_get_pair, lkrt_lkmap_str_i64_len, lkrt_lkmap_str_i64_new, lkrt_lkmap_str_i64_set,
    lkrt_lkmap_str_i64_set_ik, lkrt_lkmap_str_i64_without,
};
pub use lkmap::{
    lkrt_lkmap_str_bool_delete, lkrt_lkmap_str_bool_iter_pairs, lkrt_lkmap_str_bool_keys, lkrt_lkmap_str_bool_values,
    lkrt_lkmap_str_dyn_delete, lkrt_lkmap_str_dyn_iter_pairs, lkrt_lkmap_str_dyn_keys, lkrt_lkmap_str_dyn_values,
    lkrt_lkmap_str_f64_delete, lkrt_lkmap_str_f64_iter_pairs, lkrt_lkmap_str_f64_keys, lkrt_lkmap_str_f64_values,
    lkrt_lkmap_str_i64_delete, lkrt_lkmap_str_i64_iter_pairs, lkrt_lkmap_str_i64_keys, lkrt_lkmap_str_i64_values,
};
pub use lkset::{
    lkrt_lkset_add, lkrt_lkset_clear, lkrt_lkset_delete, lkrt_lkset_from_i64_list, lkrt_lkset_from_str_list,
    lkrt_lkset_has, lkrt_lkset_len, lkrt_lkset_new,
};
pub use lkstr::{
    lkrt_bool_to_str, lkrt_f64_to_str, lkrt_i64_to_str, lkrt_str_byte_len, lkrt_str_capitalize, lkrt_str_char_at,
    lkrt_str_char_len, lkrt_str_chars, lkrt_str_cmp, lkrt_str_concat, lkrt_str_concat_i64, lkrt_str_contains,
    lkrt_str_count, lkrt_str_ends_with, lkrt_str_find, lkrt_str_lower, lkrt_str_repeat, lkrt_str_replace,
    lkrt_str_reverse, lkrt_str_slice_chars, lkrt_str_starts_with, lkrt_str_strip_prefix, lkrt_str_strip_suffix,
    lkrt_str_substring, lkrt_str_title, lkrt_str_trim, lkrt_str_upper,
};
pub use net::{
    lkrt_bytes_free, lkrt_bytes_to_string_utf8, lkrt_handle_close, lkrt_socket_addr, lkrt_tcp_close, lkrt_tcp_connect,
    lkrt_tcp_read, lkrt_tcp_write_bytes, lkrt_tcp_write_str,
};
pub use panic::{
    lkrt_rt_cell_get, lkrt_rt_cell_new, lkrt_rt_cell_set, lkrt_rt_current_error, lkrt_rt_raise_dyn, lkrt_rt_raise_msg,
    lkrt_rt_try_pop, lkrt_rt_try_push,
};
pub use vm_mirror::{
    lkrt_lkmap_lit_finish_i64_f64, lkrt_lkmap_lit_finish_i64_i64, lkrt_lkmap_lit_finish_str_bool,
    lkrt_lkmap_lit_finish_str_dyn, lkrt_lkmap_lit_finish_str_f64, lkrt_lkmap_lit_finish_str_i64, lkrt_lkmap_lit_new,
    lkrt_lkmap_lit_set,
};

/// Called by the CLI to make the Cargo dependency explicit.
pub fn link_anchor() -> u8 {
    0
}

/// Version string embedded in the static library for diagnostics.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
