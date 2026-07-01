//! Typed native runtime support for LK LLVM AOT binaries.
//!
//! This crate is intentionally not the LK VM. It may provide low-level typed
//! helpers that LLVM-generated code links against, but it must not depend on the
//! parser, compiler, `ModuleArtifact`, `VmContext`, or the bytecode executor.

mod abi;
mod containers;
mod host;
mod io;
mod net;
mod state;

pub use abi::{lkrt_abi_version, lkrt_cleanup, lkrt_error_clear, lkrt_last_error, lkrt_string_free};
pub use containers::{
    lkrt_i64_decimal_len, lkrt_list_f64_concat, lkrt_list_f64_contains, lkrt_list_f64_index_of, lkrt_list_f64_insert,
    lkrt_list_f64_pop, lkrt_list_f64_push, lkrt_list_f64_remove_at, lkrt_list_f64_reverse, lkrt_list_f64_set,
    lkrt_list_f64_slice, lkrt_list_f64_slice_range, lkrt_list_f64_sort, lkrt_list_f64_take, lkrt_list_f64_unique,
    lkrt_list_i64_concat, lkrt_list_i64_contains, lkrt_list_i64_eq, lkrt_list_i64_index_of, lkrt_list_i64_insert,
    lkrt_list_i64_pop, lkrt_list_i64_push, lkrt_list_i64_remove_at, lkrt_list_i64_reverse, lkrt_list_i64_set,
    lkrt_list_i64_slice, lkrt_list_i64_slice_range, lkrt_list_i64_sort, lkrt_list_i64_take, lkrt_list_str_concat,
    lkrt_list_str_contains, lkrt_list_str_index_of, lkrt_list_str_insert, lkrt_list_str_pop, lkrt_list_str_push,
    lkrt_list_str_remove_at, lkrt_list_str_reverse, lkrt_list_str_set, lkrt_list_str_slice, lkrt_list_str_slice_range,
    lkrt_list_str_sort, lkrt_list_str_take, lkrt_list_str_text_len, lkrt_map_i64_f64_lookup, lkrt_map_i64_f64_set,
    lkrt_map_i64_int_lookup, lkrt_map_i64_int_set, lkrt_map_i64_ptr_lookup, lkrt_map_i64_ptr_set,
    lkrt_map_str_contains, lkrt_map_str_f64_delete, lkrt_map_str_f64_lookup, lkrt_map_str_f64_set,
    lkrt_map_str_int_delete, lkrt_map_str_int_lookup, lkrt_map_str_int_set, lkrt_map_str_ptr_delete,
    lkrt_map_str_ptr_lookup, lkrt_map_str_ptr_set, lkrt_map_str_split_key,
};
pub use host::{
    lkrt_env_get, lkrt_env_get_or, lkrt_env_has, lkrt_env_remove, lkrt_env_set, lkrt_fs_canonicalize, lkrt_fs_exists,
    lkrt_fs_metadata_is_dir, lkrt_fs_metadata_is_file, lkrt_fs_metadata_len, lkrt_fs_metadata_readonly, lkrt_fs_read,
    lkrt_fs_read_dir, lkrt_fs_read_to_string, lkrt_fs_temp_dir, lkrt_fs_write_bytes, lkrt_fs_write_str, lkrt_os_clock,
    lkrt_os_epoch, lkrt_path_temp_dir, lkrt_process_cwd, lkrt_time_now_ms, lkrt_time_sleep_ms,
};
pub use io::{lkrt_io_std_flush, lkrt_io_std_read_to_string, lkrt_io_std_write};
pub use net::{
    lkrt_bytes_free, lkrt_bytes_to_string_utf8, lkrt_handle_close, lkrt_socket_addr, lkrt_tcp_close, lkrt_tcp_connect,
    lkrt_tcp_read, lkrt_tcp_write_bytes, lkrt_tcp_write_str,
};

/// Called by the CLI to make the Cargo dependency explicit.
pub fn link_anchor() -> u8 {
    0
}

/// Version string embedded in the static library for diagnostics.
pub fn version() -> &'static str {
    env!("CARGO_PKG_VERSION")
}
