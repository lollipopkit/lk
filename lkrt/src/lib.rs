//! Typed native runtime support for LK LLVM AOT binaries.
//!
//! This crate is intentionally not the LK VM. It may provide low-level typed
//! helpers that LLVM-generated code links against, but it must not depend on the
//! parser, compiler, `ModuleArtifact`, `VmContext`, or the bytecode executor.

mod abi;
mod host;
mod io;
mod net;
mod state;

pub use abi::{lkrt_abi_version, lkrt_cleanup, lkrt_error_clear, lkrt_last_error, lkrt_string_free};
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
