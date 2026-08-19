#![cfg_attr(not(feature = "std"), no_std)]
// The no_std profile compiles the same ABI surface but reaches only the
// computation subset of it: with `fs`/`net`/`io` gated out, their helpers
// (status codes, C-string conversion, the handle table's typed accessors) have
// no callers. That is the profile working as intended, not an oversight — and
// CI builds with `-D warnings`, so without this the bare-metal images fail to
// build on dead code that is dead by design. The `std` build keeps the lint,
// which is where an actually-unused helper would show up.
#![cfg_attr(not(feature = "std"), allow(dead_code))]
//! Typed native runtime support for LK LLVM AOT binaries.
//!
//! This crate is intentionally not the LK VM. It may provide low-level typed
//! helpers that LLVM-generated code links against, but it must not depend on the
//! parser, compiler, `ModuleArtifact`, `VmContext`, or the bytecode executor.

extern crate alloc;

/// `eprintln!` where there is a stderr, a no-op where there is not.
///
/// Bare metal has no standard error. These messages report link-time or
/// invariant failures that are followed by an abort, so losing the text costs
/// diagnosis, not correctness — and a board that wants them can see them by
/// building with `std` under a debugger, or by reading the abort itself.
#[macro_export]
macro_rules! rt_eprintln {
    ($($arg:tt)*) => {{
        #[cfg(feature = "std")]
        {
            std::eprintln!($($arg)*);
        }
        #[cfg(not(feature = "std"))]
        {
            // Consume the arguments so they cannot go stale unnoticed.
            let _ = format_args!($($arg)*);
        }
    }};
}

mod abi;
// The schema describes the **host** runtime: an AOT-compiled binary links a
// `std` lkrt. Without `std` this crate deliberately exports a subset (no
// channels, no sockets, no host handles), so asserting that every schema symbol
// exists is only a question with an answer there.
#[cfg(all(test, feature = "std"))]
mod abi_conformance_test;
mod arith;
#[cfg(feature = "std")]
mod chan;
mod cpu;
mod encoding;
mod hash;
#[cfg(feature = "std")]
mod host;
#[cfg(feature = "std")]
mod io;
#[cfg(not(feature = "std"))]
mod io_bare;
mod isr;
mod lkbytes;
#[cfg(feature = "std")]
mod lkclosure;
mod lkdyn;
mod lklist;
mod lkmap;
#[cfg(feature = "std")]
mod lkprocess;
#[cfg(feature = "std")]
mod lkrandom;
#[cfg(feature = "std")]
mod lkregex;
mod lkset;
mod lkslice;
mod lkstr;
#[cfg(feature = "std")]
mod net;
mod panic;
mod port;
mod state;
mod system;
mod textcodec;
#[cfg(feature = "std")]
mod uuid;
mod vm_mirror;

pub use abi::{
    lkrt_abi_version, lkrt_abort, lkrt_assert, lkrt_assert_msg, lkrt_cleanup, lkrt_error_clear, lkrt_last_error,
    lkrt_panic, lkrt_rt_begin, lkrt_string_free,
};
pub use arith::{
    lkrt_f64_div_checked, lkrt_f64_mod_checked, lkrt_i64_div_checked, lkrt_i64_mod_checked, lkrt_i64_shl_checked,
    lkrt_i64_shr_checked, lkrt_u64_div, lkrt_u64_lt, lkrt_u64_rem, lkrt_u64_shr_checked, lkrt_u64_to_f64,
};
#[cfg(feature = "std")]
pub use chan::{
    lkrt_chan_capacity, lkrt_chan_close, lkrt_chan_is_closed, lkrt_chan_len, lkrt_chan_new, lkrt_chan_recv,
    lkrt_chan_select, lkrt_chan_send, lkrt_chan_try_recv, lkrt_chan_try_send, lkrt_spawn_arg, lkrt_spawn_args_new,
    lkrt_spawn_args_push, lkrt_spawn0, lkrt_spawn1, lkrt_spawn2, lkrt_spawn3, lkrt_spawn4, lkrt_task_await,
    lkrt_time_after, lkrt_time_timeout,
};
pub use cpu::{
    lkrt_cpu_barrier, lkrt_cpu_compiler_barrier, lkrt_cpu_irq_restore, lkrt_cpu_irq_save, lkrt_cpu_timestamp,
    lkrt_cpu_wait_for_interrupt,
};
// A `cpu_*` intrinsic that lives with the interrupt stubs rather than with the
// rest of them, because it *is* one of them: `isr.rs` holds both directions of
// the same obstacle — a vector that cannot be an operand, answered by a table.
pub use encoding::{lkrt_json_parse, lkrt_json_stringify};
#[cfg(feature = "std")]
pub use encoding::{lkrt_toml_parse, lkrt_toml_stringify, lkrt_yaml_parse, lkrt_yaml_stringify};
pub use hash::{
    lkrt_hash_crc32_bytes, lkrt_hash_crc32_str, lkrt_hash_fnv64_bytes, lkrt_hash_fnv64_str, lkrt_hash_sha1_bytes,
    lkrt_hash_sha1_str, lkrt_hash_sha256_bytes, lkrt_hash_sha256_str,
};
pub use isr::lkrt_cpu_raise_interrupt;
pub use lkbytes::{
    lkrt_lkbytes_concat, lkrt_lkbytes_contains, lkrt_lkbytes_count, lkrt_lkbytes_eq, lkrt_lkbytes_from_i64_list,
    lkrt_lkbytes_from_str, lkrt_lkbytes_get, lkrt_lkbytes_index_of, lkrt_lkbytes_is_empty, lkrt_lkbytes_len,
    lkrt_lkbytes_max, lkrt_lkbytes_min, lkrt_lkbytes_reverse, lkrt_lkbytes_skip, lkrt_lkbytes_slice, lkrt_lkbytes_sort,
    lkrt_lkbytes_sum, lkrt_lkbytes_take, lkrt_lkbytes_to_i64_list, lkrt_lkbytes_to_str, lkrt_lkbytes_unique,
    lkrt_lkbytes_utf8, lkrt_lkbytes_utf8_lossy,
};
#[cfg(feature = "std")]
pub use lkprocess::{
    lkrt_process_exit, lkrt_process_id, lkrt_process_output, lkrt_process_output_noargs, lkrt_process_output_string,
    lkrt_process_output_string_noargs, lkrt_process_set_cwd, lkrt_process_status, lkrt_process_status_noargs,
};
#[cfg(feature = "std")]
pub use lkrandom::{
    lkrt_random_bool, lkrt_random_bool_p, lkrt_random_bytes, lkrt_random_choice_dyn, lkrt_random_choice_f64,
    lkrt_random_choice_i64, lkrt_random_choice_str, lkrt_random_float, lkrt_random_int, lkrt_random_shuffle_dyn,
    lkrt_random_shuffle_f64, lkrt_random_shuffle_i64, lkrt_random_shuffle_str,
};
#[cfg(feature = "std")]
pub use lkregex::{
    lkrt_regex_captures, lkrt_regex_find, lkrt_regex_find_all, lkrt_regex_is_match, lkrt_regex_replace,
    lkrt_regex_split,
};
pub use textcodec::{
    lkrt_base64_decode, lkrt_base64_encode, lkrt_base64_encode_bytes, lkrt_hex_decode, lkrt_hex_encode,
    lkrt_hex_encode_bytes, lkrt_url_decode_component, lkrt_url_encode_component,
};
#[cfg(feature = "std")]
pub use uuid::{lkrt_uuid_is_valid, lkrt_uuid_parse, lkrt_uuid_v4};
// Re-exported at the crate root because the ABI conformance macro checks
// signatures as `crate::$symbol`.
#[cfg(feature = "std")]
pub use host::{
    lkrt_datetime_day_of_week, lkrt_datetime_day_of_year, lkrt_datetime_format, lkrt_datetime_is_weekend,
    lkrt_datetime_now, lkrt_datetime_parse, lkrt_env_get, lkrt_env_get_or, lkrt_env_has, lkrt_env_vars_map,
    lkrt_fs_append_bytes, lkrt_fs_append_str, lkrt_fs_canonicalize, lkrt_fs_copy, lkrt_fs_create_dir,
    lkrt_fs_create_dir_all, lkrt_fs_exists, lkrt_fs_is_dir, lkrt_fs_is_file, lkrt_fs_metadata_is_dir,
    lkrt_fs_metadata_is_file, lkrt_fs_metadata_len, lkrt_fs_metadata_map, lkrt_fs_metadata_readonly, lkrt_fs_read,
    lkrt_fs_read_dir_list, lkrt_fs_read_to_string, lkrt_fs_remove_dir, lkrt_fs_remove_dir_all, lkrt_fs_remove_file,
    lkrt_fs_rename, lkrt_fs_temp_dir, lkrt_fs_write_bytes, lkrt_fs_write_str, lkrt_math_acos, lkrt_math_asin,
    lkrt_math_atan, lkrt_math_atan2, lkrt_math_ceil, lkrt_math_clamp_i64, lkrt_math_cos, lkrt_math_exp,
    lkrt_math_floor, lkrt_math_log, lkrt_math_log2, lkrt_math_log10, lkrt_math_pow, lkrt_math_round, lkrt_math_sin,
    lkrt_math_sqrt, lkrt_math_tan, lkrt_os_arch, lkrt_os_clock, lkrt_os_epoch, lkrt_os_hostname, lkrt_os_name,
    lkrt_path_temp_dir, lkrt_process_cwd, lkrt_time_now_ms, lkrt_time_sleep_ms,
};
#[cfg(feature = "std")]
pub use host::{
    lkrt_math_cbrt, lkrt_math_hypot, lkrt_math_is_nan, lkrt_math_sign_f64, lkrt_math_sign_i64, lkrt_path_components,
    lkrt_path_delimiter, lkrt_path_extension, lkrt_path_file_name, lkrt_path_file_stem, lkrt_path_is_absolute,
    lkrt_path_parent, lkrt_path_sep, lkrt_path_with_extension,
};
#[cfg(feature = "std")]
pub use io::{lkrt_io_std_flush, lkrt_io_std_read_to_string, lkrt_io_std_write};
#[cfg(not(feature = "std"))]
pub use io_bare::{lkrt_io_std_flush, lkrt_io_std_read_to_string, lkrt_io_std_write, set_output};
#[cfg(feature = "std")]
pub use lkclosure::{lkrt_closure_arity, lkrt_closure_call, lkrt_closure_call_property, lkrt_closure_new};
pub use lkdyn::lkrt_dyn_from_typed_map;
pub use lkdyn::{
    DYN_BOOL, DYN_F64, DYN_I64, DYN_LIST, DYN_MAP, DYN_NIL, DYN_STR, LkDyn, lkrt_check_declared_field,
    lkrt_check_marked_field, lkrt_check_marked_field_dyn, lkrt_dyn_add, lkrt_dyn_as_bool, lkrt_dyn_as_f64,
    lkrt_dyn_as_i64, lkrt_dyn_as_key_i64, lkrt_dyn_as_key_str, lkrt_dyn_as_list, lkrt_dyn_as_map, lkrt_dyn_as_slice,
    lkrt_dyn_as_str, lkrt_dyn_cast_to_i64, lkrt_dyn_display, lkrt_dyn_display_quoted, lkrt_dyn_div, lkrt_dyn_eq,
    lkrt_dyn_field, lkrt_dyn_field_at, lkrt_dyn_from_bool, lkrt_dyn_from_bytes, lkrt_dyn_from_f64, lkrt_dyn_from_i64,
    lkrt_dyn_from_list, lkrt_dyn_from_map, lkrt_dyn_from_maybe_bool, lkrt_dyn_from_maybe_f64, lkrt_dyn_from_maybe_i64,
    lkrt_dyn_from_maybe_str, lkrt_dyn_from_nil, lkrt_dyn_from_set, lkrt_dyn_from_slice, lkrt_dyn_from_str, lkrt_dyn_ge,
    lkrt_dyn_get, lkrt_dyn_gt, lkrt_dyn_index, lkrt_dyn_index_set, lkrt_dyn_le, lkrt_dyn_len_of, lkrt_dyn_lt,
    lkrt_dyn_map_delete, lkrt_dyn_map_has, lkrt_dyn_map_keys, lkrt_dyn_map_pairs, lkrt_dyn_map_values,
    lkrt_dyn_method_missing, lkrt_dyn_mod, lkrt_dyn_mul, lkrt_dyn_neg, lkrt_dyn_not, lkrt_dyn_obj_type_id,
    lkrt_dyn_sub, lkrt_dyn_tag, lkrt_dyn_to_iter, lkrt_dyn_truthy, lkrt_dyn_type_name, lkrt_lklist_dyn_at,
    lkrt_lklist_dyn_chain, lkrt_lklist_dyn_chunk, lkrt_lklist_dyn_contains, lkrt_lklist_dyn_display,
    lkrt_lklist_dyn_enumerate, lkrt_lklist_dyn_eq, lkrt_lklist_dyn_filter_fn, lkrt_lklist_dyn_flatten,
    lkrt_lklist_dyn_join, lkrt_lklist_dyn_len, lkrt_lklist_dyn_map_fn, lkrt_lklist_dyn_new, lkrt_lklist_dyn_push,
    lkrt_lklist_dyn_reduce_fn, lkrt_lklist_dyn_set, lkrt_lklist_dyn_slice, lkrt_lklist_dyn_slice_from,
    lkrt_lklist_dyn_unique, lkrt_lklist_dyn_zip, lkrt_lklist_f64_to_dyn, lkrt_lklist_i64_to_dyn,
    lkrt_lklist_str_to_dyn, lkrt_lkmap_obj_mark, lkrt_struct_type_begin, lkrt_struct_type_field,
};
pub use lkdyn::{
    lkrt_dyn_contains, lkrt_dyn_from_typed_list, lkrt_dyn_is_list, lkrt_dyn_is_map, lkrt_dyn_list_push,
    lkrt_dyn_seq_contains,
};
pub use lklist::{
    LkMaybeF64, LkMaybeI64, LkMaybeStr, lkrt_lklist_dyn_clear, lkrt_lklist_dyn_drop_last, lkrt_lklist_dyn_index_of,
    lkrt_lklist_dyn_insert, lkrt_lklist_dyn_remove_at, lkrt_lklist_dyn_reverse, lkrt_lklist_dyn_skip,
    lkrt_lklist_dyn_take, lkrt_lklist_f64_at, lkrt_lklist_f64_chain, lkrt_lklist_f64_clear, lkrt_lklist_f64_contains,
    lkrt_lklist_f64_contains_i64, lkrt_lklist_f64_count, lkrt_lklist_f64_display, lkrt_lklist_f64_drop_last,
    lkrt_lklist_f64_eq, lkrt_lklist_f64_get_pair, lkrt_lklist_f64_index_of, lkrt_lklist_f64_insert,
    lkrt_lklist_f64_join, lkrt_lklist_f64_len, lkrt_lklist_f64_max, lkrt_lklist_f64_min, lkrt_lklist_f64_new,
    lkrt_lklist_f64_push, lkrt_lklist_f64_remove_at, lkrt_lklist_f64_reverse, lkrt_lklist_f64_set,
    lkrt_lklist_f64_skip, lkrt_lklist_f64_slice, lkrt_lklist_f64_slice_from, lkrt_lklist_f64_sort, lkrt_lklist_f64_sum,
    lkrt_lklist_f64_take, lkrt_lklist_i64_at, lkrt_lklist_i64_chain, lkrt_lklist_i64_clear, lkrt_lklist_i64_contains,
    lkrt_lklist_i64_contains_f64, lkrt_lklist_i64_count, lkrt_lklist_i64_display, lkrt_lklist_i64_drop_last,
    lkrt_lklist_i64_eq, lkrt_lklist_i64_f64_eq, lkrt_lklist_i64_filter_fn, lkrt_lklist_i64_from_range,
    lkrt_lklist_i64_get, lkrt_lklist_i64_get_pair, lkrt_lklist_i64_index_of, lkrt_lklist_i64_insert,
    lkrt_lklist_i64_join, lkrt_lklist_i64_len, lkrt_lklist_i64_map_fn, lkrt_lklist_i64_max, lkrt_lklist_i64_min,
    lkrt_lklist_i64_new, lkrt_lklist_i64_push, lkrt_lklist_i64_reduce_fn, lkrt_lklist_i64_remove_at,
    lkrt_lklist_i64_reverse, lkrt_lklist_i64_set, lkrt_lklist_i64_skip, lkrt_lklist_i64_slice,
    lkrt_lklist_i64_slice_from, lkrt_lklist_i64_sort, lkrt_lklist_i64_sum, lkrt_lklist_i64_take,
    lkrt_lklist_i64_unique, lkrt_lklist_str_at, lkrt_lklist_str_chain, lkrt_lklist_str_clear, lkrt_lklist_str_contains,
    lkrt_lklist_str_display, lkrt_lklist_str_drop_last, lkrt_lklist_str_eq, lkrt_lklist_str_filter_fn,
    lkrt_lklist_str_get_pair, lkrt_lklist_str_index_of, lkrt_lklist_str_insert, lkrt_lklist_str_join,
    lkrt_lklist_str_len, lkrt_lklist_str_map_fn, lkrt_lklist_str_max, lkrt_lklist_str_min, lkrt_lklist_str_new,
    lkrt_lklist_str_push, lkrt_lklist_str_remove_at, lkrt_lklist_str_reverse, lkrt_lklist_str_set,
    lkrt_lklist_str_skip, lkrt_lklist_str_slice, lkrt_lklist_str_slice_from, lkrt_lklist_str_sort,
    lkrt_lklist_str_take, lkrt_maybe_f64_unwrap, lkrt_maybe_i64_unwrap, lkrt_maybe_str_unwrap, lkrt_str_split,
};
pub use lkmap::{
    lkrt_lkmap_i64_f64_clear, lkrt_lkmap_i64_f64_get_pair, lkrt_lkmap_i64_f64_len, lkrt_lkmap_i64_f64_new,
    lkrt_lkmap_i64_f64_set, lkrt_lkmap_i64_i64_clear, lkrt_lkmap_i64_i64_get_pair, lkrt_lkmap_i64_i64_len,
    lkrt_lkmap_i64_i64_new, lkrt_lkmap_i64_i64_set, lkrt_lkmap_str_dyn_get, lkrt_lkmap_str_dyn_get_at,
    lkrt_lkmap_str_dyn_has, lkrt_lkmap_str_dyn_len, lkrt_lkmap_str_dyn_merge, lkrt_lkmap_str_dyn_merge_typed,
    lkrt_lkmap_str_dyn_new, lkrt_lkmap_str_dyn_rebuild, lkrt_lkmap_str_dyn_set, lkrt_lkmap_str_dyn_without,
    lkrt_lkmap_str_f64_get_pair, lkrt_lkmap_str_f64_len, lkrt_lkmap_str_f64_new, lkrt_lkmap_str_f64_set,
    lkrt_lkmap_str_f64_set_ik, lkrt_lkmap_str_f64_without, lkrt_lkmap_str_i64_get_pair, lkrt_lkmap_str_i64_len,
    lkrt_lkmap_str_i64_new, lkrt_lkmap_str_i64_set, lkrt_lkmap_str_i64_set_ik, lkrt_lkmap_str_i64_without,
};
pub use lkmap::{
    lkrt_lkmap_i64_f64_display, lkrt_lkmap_i64_f64_iter_pairs, lkrt_lkmap_i64_f64_keys, lkrt_lkmap_i64_f64_values,
    lkrt_lkmap_i64_i64_display, lkrt_lkmap_i64_i64_iter_pairs, lkrt_lkmap_i64_i64_keys, lkrt_lkmap_i64_i64_values,
    lkrt_lkmap_str_bool_display, lkrt_lkmap_str_f64_display, lkrt_lkmap_str_i64_display,
};
pub use lkmap::{
    lkrt_lkmap_str_bool_delete, lkrt_lkmap_str_bool_iter_pairs, lkrt_lkmap_str_bool_keys, lkrt_lkmap_str_bool_values,
    lkrt_lkmap_str_dyn_clear, lkrt_lkmap_str_dyn_delete, lkrt_lkmap_str_dyn_iter_pairs, lkrt_lkmap_str_dyn_keys,
    lkrt_lkmap_str_dyn_values, lkrt_lkmap_str_f64_clear, lkrt_lkmap_str_f64_delete, lkrt_lkmap_str_f64_iter_pairs,
    lkrt_lkmap_str_f64_keys, lkrt_lkmap_str_f64_values, lkrt_lkmap_str_i64_clear, lkrt_lkmap_str_i64_delete,
    lkrt_lkmap_str_i64_iter_pairs, lkrt_lkmap_str_i64_keys, lkrt_lkmap_str_i64_values,
};
pub use lkset::{
    lkrt_lkset_add, lkrt_lkset_clear, lkrt_lkset_combine, lkrt_lkset_delete, lkrt_lkset_display, lkrt_lkset_eq,
    lkrt_lkset_from_dyn_list, lkrt_lkset_from_i64_list, lkrt_lkset_from_str_list, lkrt_lkset_has, lkrt_lkset_iter,
    lkrt_lkset_len, lkrt_lkset_new, lkrt_lkset_relate,
};
pub use lkslice::{
    lkrt_lkslice_i64_contains, lkrt_lkslice_i64_count, lkrt_lkslice_i64_display, lkrt_lkslice_i64_get_pair,
    lkrt_lkslice_i64_index_of, lkrt_lkslice_i64_is_empty, lkrt_lkslice_i64_len, lkrt_lkslice_i64_max,
    lkrt_lkslice_i64_min, lkrt_lkslice_i64_new, lkrt_lkslice_i64_skip, lkrt_lkslice_i64_sub, lkrt_lkslice_i64_sum,
    lkrt_lkslice_i64_take, lkrt_lkslice_i64_to_list,
};
pub use lkstr::{
    lkrt_bool_to_str, lkrt_f64_to_str, lkrt_i64_to_str, lkrt_str_byte_at, lkrt_str_byte_len, lkrt_str_capitalize,
    lkrt_str_char_at, lkrt_str_char_len, lkrt_str_chars, lkrt_str_cmp, lkrt_str_concat, lkrt_str_concat_i64,
    lkrt_str_contains, lkrt_str_count, lkrt_str_ends_with, lkrt_str_index_of, lkrt_str_lower, lkrt_str_pad_left,
    lkrt_str_pad_right, lkrt_str_repeat, lkrt_str_replace, lkrt_str_reverse, lkrt_str_skip, lkrt_str_slice_chars,
    lkrt_str_starts_with, lkrt_str_strip, lkrt_str_strip_prefix, lkrt_str_strip_suffix, lkrt_str_take, lkrt_str_title,
    lkrt_str_to_float, lkrt_str_to_int, lkrt_str_trim, lkrt_str_upper, lkrt_u64_to_str,
};
#[cfg(feature = "std")]
pub use net::{
    lkrt_handle_close, lkrt_socket_addr, lkrt_tcp_close, lkrt_tcp_connect, lkrt_tcp_read, lkrt_tcp_write_bytes,
    lkrt_tcp_write_str,
};
// The closure-callback entries are `std`-only, like `lkclosure` itself: a
// closure value is deep-copied through the channel model, which needs an OS.
// Listed apart rather than inside the block above, because a `cfg` cannot sit
// on one name in a `use` list.
#[cfg(feature = "std")]
pub use lkdyn::{lkrt_lklist_dyn_filter_closure, lkrt_lklist_dyn_map_closure, lkrt_lklist_dyn_reduce_closure};
pub use panic::{
    lkrt_rt_cell_get, lkrt_rt_cell_get_raw, lkrt_rt_cell_new, lkrt_rt_cell_new_raw, lkrt_rt_cell_set,
    lkrt_rt_cell_set_raw, lkrt_rt_current_error, lkrt_rt_handle_release, lkrt_rt_handle_release_deep,
    lkrt_rt_maybe_guard, lkrt_rt_raise_dyn, lkrt_rt_raise_msg, lkrt_rt_try_pop, lkrt_rt_try_push,
};
pub use port::{
    lkrt_port_in_u8, lkrt_port_in_u16, lkrt_port_in_u32, lkrt_port_out_u8, lkrt_port_out_u16, lkrt_port_out_u32,
};
pub use system::{
    lkrt_cpu_invalidate_page, lkrt_cpu_load_gdt, lkrt_cpu_load_idt, lkrt_cpu_load_task_register, lkrt_cpu_read_cr2,
    lkrt_cpu_read_cr3, lkrt_cpu_reload_segments, lkrt_cpu_write_cr3,
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
