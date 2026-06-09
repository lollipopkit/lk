//! Registry for host primitives that LLVM AOT may call through `lkrt`.
//!
//! This is metadata only. Full stdlib method bodies should live in LK stdlib
//! source or in compile-time constant evaluation, not as scattered LLVM matches.

#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::llvm) enum NativeIntrinsicEffect {
    Pure,
    ReadsHost,
    WritesHost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::llvm) enum NativeIntrinsicType {
    I64,
    F64,
    Ptr,
    StrPtr,
    Nil,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(in crate::llvm) struct NativeIntrinsic {
    pub module: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub params: &'static [NativeIntrinsicType],
    pub result: NativeIntrinsicType,
    pub effect: NativeIntrinsicEffect,
}

pub(in crate::llvm) const NATIVE_INTRINSICS: &[NativeIntrinsic] = &[
    NativeIntrinsic {
        module: "lkrt",
        name: "abi_version",
        symbol: "lkrt_abi_version",
        params: &[],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::Pure,
    },
    NativeIntrinsic {
        module: "lkrt",
        name: "cleanup",
        symbol: "lkrt_cleanup",
        params: &[],
        result: NativeIntrinsicType::Nil,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "lkrt",
        name: "error_clear",
        symbol: "lkrt_error_clear",
        params: &[],
        result: NativeIntrinsicType::Nil,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "lkrt",
        name: "last_error",
        symbol: "lkrt_last_error",
        params: &[],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "lkrt",
        name: "string_free",
        symbol: "lkrt_string_free",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::Nil,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "socket",
        name: "addr",
        symbol: "lkrt_socket_addr",
        params: &[NativeIntrinsicType::StrPtr, NativeIntrinsicType::I64],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::Pure,
    },
    NativeIntrinsic {
        module: "tcp",
        name: "connect",
        symbol: "lkrt_tcp_connect",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "tcp",
        name: "read",
        symbol: "lkrt_tcp_read",
        params: &[NativeIntrinsicType::I64, NativeIntrinsicType::I64],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "tcp",
        name: "write_str",
        symbol: "lkrt_tcp_write_str",
        params: &[NativeIntrinsicType::I64, NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "tcp",
        name: "write_bytes",
        symbol: "lkrt_tcp_write_bytes",
        params: &[NativeIntrinsicType::I64, NativeIntrinsicType::I64],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "tcp",
        name: "close",
        symbol: "lkrt_tcp_close",
        params: &[NativeIntrinsicType::I64],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "bytes",
        name: "to_string_utf8",
        symbol: "lkrt_bytes_to_string_utf8",
        params: &[NativeIntrinsicType::I64],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::Pure,
    },
    NativeIntrinsic {
        module: "bytes",
        name: "free",
        symbol: "lkrt_bytes_free",
        params: &[NativeIntrinsicType::I64],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "lkrt",
        name: "handle_close",
        symbol: "lkrt_handle_close",
        params: &[NativeIntrinsicType::I64],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "io.std",
        name: "write",
        symbol: "lkrt_io_std_write",
        params: &[
            NativeIntrinsicType::I64,
            NativeIntrinsicType::StrPtr,
            NativeIntrinsicType::I64,
        ],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "io.std",
        name: "flush",
        symbol: "lkrt_io_std_flush",
        params: &[NativeIntrinsicType::I64],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "io.std",
        name: "read_to_string",
        symbol: "lkrt_io_std_read_to_string",
        params: &[NativeIntrinsicType::I64],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "env",
        name: "get",
        symbol: "lkrt_env_get",
        params: &[NativeIntrinsicType::StrPtr, NativeIntrinsicType::Ptr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "env",
        name: "get_or",
        symbol: "lkrt_env_get_or",
        params: &[NativeIntrinsicType::StrPtr, NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "env",
        name: "has",
        symbol: "lkrt_env_has",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "env",
        name: "set",
        symbol: "lkrt_env_set",
        params: &[NativeIntrinsicType::StrPtr, NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "env",
        name: "remove",
        symbol: "lkrt_env_remove",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "read",
        symbol: "lkrt_fs_read",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "read_to_string",
        symbol: "lkrt_fs_read_to_string",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "write_str",
        symbol: "lkrt_fs_write_str",
        params: &[NativeIntrinsicType::StrPtr, NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "write_bytes",
        symbol: "lkrt_fs_write_bytes",
        params: &[NativeIntrinsicType::StrPtr, NativeIntrinsicType::I64],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::WritesHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "exists",
        symbol: "lkrt_fs_exists",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "read_dir",
        symbol: "lkrt_fs_read_dir",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "metadata_len",
        symbol: "lkrt_fs_metadata_len",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "metadata_is_file",
        symbol: "lkrt_fs_metadata_is_file",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "metadata_is_dir",
        symbol: "lkrt_fs_metadata_is_dir",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "metadata_readonly",
        symbol: "lkrt_fs_metadata_readonly",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "canonicalize",
        symbol: "lkrt_fs_canonicalize",
        params: &[NativeIntrinsicType::StrPtr],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "fs",
        name: "temp_dir",
        symbol: "lkrt_fs_temp_dir",
        params: &[],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "path",
        name: "temp_dir",
        symbol: "lkrt_path_temp_dir",
        params: &[],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "process",
        name: "cwd",
        symbol: "lkrt_process_cwd",
        params: &[],
        result: NativeIntrinsicType::StrPtr,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "os",
        name: "clock",
        symbol: "lkrt_os_clock",
        params: &[],
        result: NativeIntrinsicType::F64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "os",
        name: "epoch",
        symbol: "lkrt_os_epoch",
        params: &[],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "time",
        name: "now",
        symbol: "lkrt_time_now_ms",
        params: &[],
        result: NativeIntrinsicType::I64,
        effect: NativeIntrinsicEffect::ReadsHost,
    },
    NativeIntrinsic {
        module: "time",
        name: "sleep",
        symbol: "lkrt_time_sleep_ms",
        params: &[NativeIntrinsicType::I64],
        result: NativeIntrinsicType::Nil,
        effect: NativeIntrinsicEffect::WritesHost,
    },
];

pub(in crate::llvm) fn native_intrinsic(module: &str, name: &str) -> Option<&'static NativeIntrinsic> {
    NATIVE_INTRINSICS
        .iter()
        .find(|intrinsic| intrinsic.module == module && intrinsic.name == name)
}

pub(in crate::llvm) fn native_intrinsic_declarations() -> String {
    let mut declarations = String::new();
    for intrinsic in NATIVE_INTRINSICS {
        if !intrinsic.symbol.starts_with("lkrt_") {
            continue;
        }
        declarations.push_str("declare ");
        declarations.push_str(llvm_type(intrinsic.result));
        declarations.push_str(" @");
        declarations.push_str(intrinsic.symbol);
        declarations.push('(');
        for (index, param) in intrinsic.params.iter().enumerate() {
            if index > 0 {
                declarations.push_str(", ");
            }
            declarations.push_str(llvm_type(*param));
        }
        declarations.push_str(")\n");
    }
    declarations.push('\n');
    declarations
}

fn llvm_type(value: NativeIntrinsicType) -> &'static str {
    match value {
        NativeIntrinsicType::I64 => "i64",
        NativeIntrinsicType::F64 => "double",
        NativeIntrinsicType::Ptr | NativeIntrinsicType::StrPtr => "ptr",
        NativeIntrinsicType::Nil => "void",
    }
}
