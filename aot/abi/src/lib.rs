//! Single-source-of-truth ABI schema shared by the AOT codegen and `lkrt`.
//!
//! This crate is deliberately dependency-free (no `lk-core`, no `lk-stdlib`, no
//! LLVM). It only describes *what* native runtime functions exist, their typed
//! signatures, and their host-effect classification. The LLVM-specific rendering
//! of these signatures (the `declare` text) lives in the codegen crate, which
//! consumes [`ABI_FUNCTIONS`]; `lkrt` links the implementations and shares
//! [`ABI_VERSION`]. Keeping the schema here removes the previous hand-synced
//! duplication between the codegen intrinsic table, `lkrt`'s exports, and its ABI
//! version constant.

/// Native runtime ABI version. Bumped when the calling convention or the
/// representation contract (present-bit, ownership, handle layout) changes.
/// A native binary whose linked `lkrt` reports a different value is a
/// link/configuration error, never a reason to fall back to the VM.
pub const ABI_VERSION: i64 = 1;

/// How a native intrinsic interacts with host state, used by codegen to decide
/// which optimizations (CSE/hoist/DCE) are sound.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiEffect {
    Pure,
    ReadsHost,
    WritesHost,
}

/// The typed vocabulary of native ABI parameters/results. Deliberately small:
/// scalars plus opaque pointers. `StrPtr` is a `*const c_char`; `Ptr` is any
/// other raw pointer (buffers, out-params, handles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbiType {
    I64,
    F64,
    Ptr,
    StrPtr,
    Nil,
    /// The boxed dynamic value carrier (`LkDyn { tag, payload }`), passed by
    /// value as LLVM `{ i64, i64 }` — same shape as the `Maybe` carriers.
    DynVal,
}

/// One native runtime function: its module/name identity (as referenced by the
/// lowering), its exported C symbol, and its typed signature + effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbiFn {
    pub module: &'static str,
    pub name: &'static str,
    pub symbol: &'static str,
    pub params: &'static [AbiType],
    pub result: AbiType,
    pub effect: AbiEffect,
}

/// Invokes the given callback macro with every ABI table entry, in order. This is
/// the single source of truth (RFC aot-redesign §3.3): the [`ABI_FUNCTIONS`] const
/// table below and `lkrt`'s compile-time signature-conformance checks both expand
/// from it, so a signature can no longer drift between the schema, the codegen
/// `declare`s, and the runtime implementation without failing the build/tests.
///
/// Entry shape: `("module", "name", symbol_ident, Effect, [ParamTypes...], RetType);`
#[macro_export]
macro_rules! for_each_abi_fn {
    ($callback:ident) => {
        $callback! {
            ("lkrt", "abi_version", lkrt_abi_version, Pure, [], I64);
            ("lkrt", "abi_check", lkrt_abi_check, WritesHost, [I64], Nil);
            ("lkrt", "cleanup", lkrt_cleanup, WritesHost, [], Nil);
            ("lkrt", "error_clear", lkrt_error_clear, WritesHost, [], Nil);
            ("lkrt", "last_error", lkrt_last_error, ReadsHost, [], StrPtr);
            ("lkrt", "string_free", lkrt_string_free, WritesHost, [StrPtr], Nil);
            // Fatal-guard abort: flushes C stdio before aborting so a guard firing
            // after user output does not discard what the program already printed.
            ("lkrt", "abort", lkrt_abort, WritesHost, [], Nil);
            // Runtime builtins lowered from `GetGlobal` + `Call` shapes. `assert`
            // aborts loudly on a false condition, matching the VM's fatal error.
            ("rt", "assert", lkrt_assert, WritesHost, [I64], Nil);
            ("rt", "assert_msg", lkrt_assert_msg, WritesHost, [I64, StrPtr], Nil);
            ("rt", "panic", lkrt_panic, WritesHost, [StrPtr], Nil);
            ("socket", "addr", lkrt_socket_addr, Pure, [StrPtr, I64], StrPtr);
            ("tcp", "connect", lkrt_tcp_connect, WritesHost, [StrPtr], I64);
            ("tcp", "read", lkrt_tcp_read, WritesHost, [I64, I64], I64);
            ("tcp", "write_str", lkrt_tcp_write_str, WritesHost, [I64, StrPtr], I64);
            ("tcp", "write_bytes", lkrt_tcp_write_bytes, WritesHost, [I64, I64], I64);
            ("tcp", "close", lkrt_tcp_close, WritesHost, [I64], I64);
            ("bytes", "to_string_utf8", lkrt_bytes_to_string_utf8, Pure, [I64], StrPtr);
            ("bytes", "free", lkrt_bytes_free, WritesHost, [I64], I64);
            ("lkrt", "handle_close", lkrt_handle_close, WritesHost, [I64], I64);
            ("io.std", "write", lkrt_io_std_write, WritesHost, [I64, StrPtr, I64], I64);
            ("io.std", "flush", lkrt_io_std_flush, WritesHost, [I64], I64);
            ("io.std", "read_to_string", lkrt_io_std_read_to_string, WritesHost, [I64], StrPtr);
            ("env", "get", lkrt_env_get, ReadsHost, [StrPtr, Ptr], I64);
            ("env", "get_or", lkrt_env_get_or, ReadsHost, [StrPtr, StrPtr], StrPtr);
            ("env", "has", lkrt_env_has, ReadsHost, [StrPtr], I64);
            ("env", "set", lkrt_env_set, WritesHost, [StrPtr, StrPtr], I64);
            ("env", "remove", lkrt_env_remove, WritesHost, [StrPtr], I64);
            ("fs", "read", lkrt_fs_read, ReadsHost, [StrPtr], I64);
            ("fs", "read_to_string", lkrt_fs_read_to_string, ReadsHost, [StrPtr], StrPtr);
            ("fs", "write_str", lkrt_fs_write_str, WritesHost, [StrPtr, StrPtr], I64);
            ("fs", "write_bytes", lkrt_fs_write_bytes, WritesHost, [StrPtr, I64], I64);
            ("fs", "exists", lkrt_fs_exists, ReadsHost, [StrPtr], I64);
            ("fs", "metadata_len", lkrt_fs_metadata_len, ReadsHost, [StrPtr], I64);
            ("fs", "metadata_is_file", lkrt_fs_metadata_is_file, ReadsHost, [StrPtr], I64);
            ("fs", "metadata_is_dir", lkrt_fs_metadata_is_dir, ReadsHost, [StrPtr], I64);
            ("fs", "metadata_readonly", lkrt_fs_metadata_readonly, ReadsHost, [StrPtr], I64);
            ("fs", "canonicalize", lkrt_fs_canonicalize, ReadsHost, [StrPtr], StrPtr);
            ("fs", "temp_dir", lkrt_fs_temp_dir, ReadsHost, [], StrPtr);
            ("path", "temp_dir", lkrt_path_temp_dir, ReadsHost, [], StrPtr);
            ("process", "cwd", lkrt_process_cwd, ReadsHost, [], StrPtr);
            ("os", "clock", lkrt_os_clock, ReadsHost, [], F64);
            ("os", "epoch", lkrt_os_epoch, ReadsHost, [], I64);
            ("os", "hostname", lkrt_os_hostname, ReadsHost, [], StrPtr);
            ("os", "arch", lkrt_os_arch, ReadsHost, [], StrPtr);
            // The module member is `os.os` (renamed: the schema name pairs with
            // the exported symbol, not the LK-visible member).
            ("os", "name", lkrt_os_name, ReadsHost, [], StrPtr);
            // Sorted UTF-8 entry names as a List<str> handle (VM-exact).
            ("fs", "read_dir_list", lkrt_fs_read_dir_list, ReadsHost, [StrPtr], Ptr);
            // `math.floor(Float) -> Int` with the VM's exact rounding (`floor()
            // as i64`, saturating); an `Int` argument short-circuits in the lowering.
            ("math", "floor", lkrt_math_floor, Pure, [F64], I64);
            ("math", "ceil", lkrt_math_ceil, Pure, [F64], I64);
            ("math", "round", lkrt_math_round, Pure, [F64], I64);
            // Aborts on a negative argument (the stdlib module's loud error),
            // so it must never be treated as removable pure math.
            ("math", "sqrt", lkrt_math_sqrt, ReadsHost, [F64], F64);
            ("math", "sin", lkrt_math_sin, Pure, [F64], F64);
            ("math", "cos", lkrt_math_cos, Pure, [F64], F64);
            ("math", "exp", lkrt_math_exp, Pure, [F64], F64);
            ("math", "pow", lkrt_math_pow, Pure, [F64, F64], F64);
            // chrono-backed datetime (same crate as the stdlib module, so
            // formatting/weekday output is byte-identical). `format`/`parse`/
            // ordinal helpers abort on invalid input like the VM's loud error.
            ("datetime", "now", lkrt_datetime_now, ReadsHost, [], I64);
            ("datetime", "format", lkrt_datetime_format, ReadsHost, [I64, StrPtr], StrPtr);
            ("datetime", "parse", lkrt_datetime_parse, ReadsHost, [StrPtr, StrPtr], I64);
            ("datetime", "day_of_week", lkrt_datetime_day_of_week, ReadsHost, [I64], I64);
            ("datetime", "day_of_year", lkrt_datetime_day_of_year, ReadsHost, [I64], I64);
            ("datetime", "is_weekend", lkrt_datetime_is_weekend, ReadsHost, [I64], I64);
            ("time", "now", lkrt_time_now_ms, ReadsHost, [], I64);
            ("time", "sleep", lkrt_time_sleep_ms, WritesHost, [I64], Nil);
            // Growable `List<i64>` handles (Phase 2 container handle-ification). `new`
            // allocates a handle, `push` appends, `len` counts, `get` indexes with VM
            // semantics (negative-from-end; out-of-range writes `present = 0`).
            ("list_h", "i64_new", lkrt_lklist_i64_new, WritesHost, [], Ptr);
            ("list_h", "i64_from_range", lkrt_lklist_i64_from_range, WritesHost, [I64, I64, I64, I64], Ptr);
            ("list_h", "i64_take", lkrt_lklist_i64_take, WritesHost, [Ptr, I64], Ptr);
            ("list_h", "i64_skip", lkrt_lklist_i64_skip, WritesHost, [Ptr, I64], Ptr);
            ("list_h", "i64_chain", lkrt_lklist_i64_chain, WritesHost, [Ptr, Ptr], Ptr);
            ("list_h", "i64_push", lkrt_lklist_i64_push, WritesHost, [Ptr, I64], Nil);
            // List HOF over compiled zero-capture lambdas (`ptr @lk_fn_N`
            // callbacks). The callback may abort (div/0 inside the lambda), so
            // none of these are Pure.
            // VM-exact list display text (`[1,2,3]`), arena-owned.
            ("list_h", "i64_display", lkrt_lklist_i64_display, WritesHost, [Ptr], StrPtr);
            ("list_h", "f64_display", lkrt_lklist_f64_display, WritesHost, [Ptr], StrPtr);
            ("list_h", "str_display", lkrt_lklist_str_display, WritesHost, [Ptr], StrPtr);
            // Structural equality (1/0): same length + element-wise `==`;
            // `i64_f64_eq` compares Int against Float lists with numeric
            // coercion (`[1] == [1.0]` is true in the VM).
            ("list_h", "i64_eq", lkrt_lklist_i64_eq, ReadsHost, [Ptr, Ptr], I64);
            ("list_h", "f64_eq", lkrt_lklist_f64_eq, ReadsHost, [Ptr, Ptr], I64);
            ("list_h", "i64_f64_eq", lkrt_lklist_i64_f64_eq, ReadsHost, [Ptr, Ptr], I64);
            ("list_h", "str_eq", lkrt_lklist_str_eq, ReadsHost, [Ptr, Ptr], I64);
            ("list_h", "i64_map_fn", lkrt_lklist_i64_map_fn, WritesHost, [Ptr, Ptr], Ptr);
            ("list_h", "i64_filter_fn", lkrt_lklist_i64_filter_fn, WritesHost, [Ptr, Ptr], Ptr);
            ("list_h", "i64_reduce_fn", lkrt_lklist_i64_reduce_fn, WritesHost, [Ptr, I64, Ptr], I64);
            ("list_h", "i64_len", lkrt_lklist_i64_len, ReadsHost, [Ptr], I64);
            ("list_h", "i64_get", lkrt_lklist_i64_get, ReadsHost, [Ptr, I64, Ptr], I64);
            ("list_h", "i64_at", lkrt_lklist_i64_at, ReadsHost, [Ptr, I64], I64);
            // Store `list[index] = value`; aborts on an out-of-range/negative index
            // (matching the VM's fatal store-index error — a halt, not a nil).
            ("list_h", "i64_set", lkrt_lklist_i64_set, WritesHost, [Ptr, I64, I64], Nil);
            // Linear membership test; returns 0/1 (the caller narrows to `i1`).
            ("list_h", "i64_contains", lkrt_lklist_i64_contains, ReadsHost, [Ptr, I64], I64);
            // `xs[start..]`: a fresh handle with the elements from `start` on
            // (negative `start` aborts, matching the VM's fatal slice error).
            ("list_h", "i64_slice_from", lkrt_lklist_i64_slice_from, WritesHost, [Ptr, I64], Ptr);
            ("list_h", "f64_slice_from", lkrt_lklist_f64_slice_from, WritesHost, [Ptr, I64], Ptr);
            ("list_h", "str_slice_from", lkrt_lklist_str_slice_from, WritesHost, [Ptr, I64], Ptr);
            ("list_h", "f64_new", lkrt_lklist_f64_new, WritesHost, [], Ptr);
            ("list_h", "f64_push", lkrt_lklist_f64_push, WritesHost, [Ptr, F64], Nil);
            ("list_h", "f64_len", lkrt_lklist_f64_len, ReadsHost, [Ptr], I64);
            ("list_h", "f64_at", lkrt_lklist_f64_at, ReadsHost, [Ptr, I64], F64);
            ("list_h", "f64_set", lkrt_lklist_f64_set, WritesHost, [Ptr, I64, F64], Nil);
            ("list_h", "f64_contains", lkrt_lklist_f64_contains, ReadsHost, [Ptr, F64], I64);
            // String-element list handle (elements are interned string-constant pointers).
            ("list_h", "str_new", lkrt_lklist_str_new, WritesHost, [], Ptr);
            ("list_h", "str_push", lkrt_lklist_str_push, WritesHost, [Ptr, StrPtr], Nil);
            ("list_h", "str_len", lkrt_lklist_str_len, ReadsHost, [Ptr], I64);
            ("list_h", "str_at", lkrt_lklist_str_at, ReadsHost, [Ptr, I64], StrPtr);
            ("list_h", "str_join", lkrt_lklist_str_join, WritesHost, [Ptr, StrPtr], StrPtr);
            ("list_h", "str_contains", lkrt_lklist_str_contains, ReadsHost, [Ptr, StrPtr], I64);
            ("list_h", "i64_slice", lkrt_lklist_i64_slice, WritesHost, [Ptr, I64, I64], Ptr);
            // String-keyed map handle. `get_pair` (returning a by-value `Maybe<i64>`) is
            // declared directly in codegen, like the list variant.
            ("map_h", "str_i64_new", lkrt_lkmap_str_i64_new, WritesHost, [], Ptr);
            ("map_h", "str_i64_set", lkrt_lkmap_str_i64_set, WritesHost, [Ptr, StrPtr, I64], Nil);
            ("map_h", "str_i64_len", lkrt_lkmap_str_i64_len, ReadsHost, [Ptr], I64);
            // `{ ..rest }`: a fresh handle with one key removed (chained per key).
            ("map_h", "str_i64_without", lkrt_lkmap_str_i64_without, WritesHost, [Ptr, StrPtr], Ptr);
            ("map_h", "str_f64_without", lkrt_lkmap_str_f64_without, WritesHost, [Ptr, StrPtr], Ptr);
            // Int-keyed map handle. `get_pair` (by-value `Maybe<i64>`) is declared in codegen.
            ("map_h", "i64_i64_new", lkrt_lkmap_i64_i64_new, WritesHost, [], Ptr);
            ("map_h", "i64_i64_set", lkrt_lkmap_i64_i64_set, WritesHost, [Ptr, I64, I64], Nil);
            ("map_h", "i64_i64_len", lkrt_lkmap_i64_i64_len, ReadsHost, [Ptr], I64);
            // String-keyed, f64-valued map. `get_pair` (by-value `Maybe<f64>`) → codegen.
            ("map_h", "str_f64_new", lkrt_lkmap_str_f64_new, WritesHost, [], Ptr);
            ("map_h", "str_f64_set", lkrt_lkmap_str_f64_set, WritesHost, [Ptr, StrPtr, F64], Nil);
            ("map_h", "str_f64_len", lkrt_lkmap_str_f64_len, ReadsHost, [Ptr], I64);
            // Int-keyed, f64-valued map. `get_pair` (by-value `Maybe<f64>`) → codegen.
            // Composite string-int key store (`m["n${i}"] = v`): the key is built
            // on the stack inside lkrt, so the store allocates nothing on updates.
            ("map_h", "str_i64_set_ik", lkrt_lkmap_str_i64_set_ik, WritesHost, [Ptr, StrPtr, I64, I64], Nil);
            ("map_h", "str_f64_set_ik", lkrt_lkmap_str_f64_set_ik, WritesHost, [Ptr, StrPtr, I64, F64], Nil);
            ("map_h", "i64_f64_new", lkrt_lkmap_i64_f64_new, WritesHost, [], Ptr);
            ("map_h", "i64_f64_set", lkrt_lkmap_i64_f64_set, WritesHost, [Ptr, I64, F64], Nil);
            ("map_h", "i64_f64_len", lkrt_lkmap_i64_f64_len, ReadsHost, [Ptr], I64);
            // Byte-wise string comparison, returning -1/0/1 (the caller compares to 0).
            ("str", "cmp", lkrt_str_cmp, Pure, [StrPtr, StrPtr], I64);
            // `a ++ b` → a freshly allocated C string (`WritesHost`: allocates/leaks).
            ("str", "concat", lkrt_str_concat, WritesHost, [StrPtr, StrPtr], StrPtr);
            // `prefix ++ decimal(suffix)` in one allocation — the composite string-int
            // key shape proven by `GetIndexStrI`/`SetIndexStrI` facts.
            ("str", "concat_i64", lkrt_str_concat_i64, WritesHost, [StrPtr, I64], StrPtr);
            ("str", "char_len", lkrt_str_char_len, Pure, [StrPtr], I64);
            ("str", "starts_with", lkrt_str_starts_with, Pure, [StrPtr, StrPtr], I64);
            ("str", "contains", lkrt_str_contains, Pure, [StrPtr, StrPtr], I64);
            ("str", "slice_chars", lkrt_str_slice_chars, WritesHost, [StrPtr, I64, I64], StrPtr);
            ("str", "ends_with", lkrt_str_ends_with, Pure, [StrPtr, StrPtr], I64);
            ("str", "lower", lkrt_str_lower, WritesHost, [StrPtr], StrPtr);
            ("str", "upper", lkrt_str_upper, WritesHost, [StrPtr], StrPtr);
            ("str", "trim", lkrt_str_trim, WritesHost, [StrPtr], StrPtr);
            ("str", "find", lkrt_str_find, Pure, [StrPtr, StrPtr], I64);
            ("str", "substring", lkrt_str_substring, WritesHost, [StrPtr, I64, I64], StrPtr);
            ("str", "reverse", lkrt_str_reverse, WritesHost, [StrPtr], StrPtr);
            ("str", "repeat", lkrt_str_repeat, WritesHost, [StrPtr, I64], StrPtr);
            ("str", "replace", lkrt_str_replace, WritesHost, [StrPtr, StrPtr, StrPtr], StrPtr);
            ("str", "chars", lkrt_str_chars, WritesHost, [StrPtr], Ptr);
            ("str", "char_at", lkrt_str_char_at, WritesHost, [StrPtr, I64], DynVal);
            // `s.split(sep)` → a fresh `str` list handle (Rust `str::split`, so
            // VM-exact); parts are arena-owned C strings.
            ("str", "split", lkrt_str_split, WritesHost, [StrPtr, StrPtr], Ptr);
            // Scalar → display string (the VM's `ToString`), allocating/leaking a C string.
            ("str", "from_i64", lkrt_i64_to_str, WritesHost, [I64], StrPtr);
            ("str", "from_f64", lkrt_f64_to_str, WritesHost, [F64], StrPtr);
            ("str", "from_bool", lkrt_bool_to_str, WritesHost, [I64], StrPtr);
            // Divisor-guarded arithmetic: abort on a zero divisor (matching the VM's fatal
            // error) instead of raw `sdiv`/`fdiv`/`frem` UB. `ReadsHost` keeps codegen from
            // ever treating them as removable pure math (the abort is an observable effect).
            // Boxed dynamic values (`LkDyn`, plan M4.2 deep coverage): boxing,
            // guarded unboxing, VM-promotion arithmetic, equality/ordering,
            // the two display modes, and the mixed-element list family.
            ("dyn", "from_nil", lkrt_dyn_from_nil, Pure, [], DynVal);
            ("dyn", "from_bool", lkrt_dyn_from_bool, Pure, [I64], DynVal);
            ("dyn", "from_i64", lkrt_dyn_from_i64, Pure, [I64], DynVal);
            ("dyn", "from_f64", lkrt_dyn_from_f64, Pure, [F64], DynVal);
            ("dyn", "from_str", lkrt_dyn_from_str, Pure, [StrPtr], DynVal);
            ("dyn", "from_list", lkrt_dyn_from_list, Pure, [Ptr], DynVal);
            // Nullable-carrier boxing (`(value, present)` from the Maybe struct's
            // two words): present boxes the payload, absent boxes nil. Used where
            // a `Maybe` crosses a user-function call — VM call semantics pass nil
            // through, unlike the scalar-context unwrap which aborts.
            ("dyn", "from_maybe_i64", lkrt_dyn_from_maybe_i64, Pure, [I64, I64], DynVal);
            ("dyn", "from_maybe_f64", lkrt_dyn_from_maybe_f64, Pure, [F64, I64], DynVal);
            ("dyn", "from_maybe_str", lkrt_dyn_from_maybe_str, Pure, [StrPtr, I64], DynVal);
            ("dyn", "from_maybe_bool", lkrt_dyn_from_maybe_bool, Pure, [I64, I64], DynVal);
            ("dyn", "tag", lkrt_dyn_tag, Pure, [DynVal], I64);
            // VM truthiness (`truthy_unchecked`): only nil and false are falsy.
            ("dyn", "truthy", lkrt_dyn_truthy, Pure, [DynVal], I64);
            ("dyn", "as_i64", lkrt_dyn_as_i64, ReadsHost, [DynVal], I64);
            ("dyn", "as_f64", lkrt_dyn_as_f64, ReadsHost, [DynVal], F64);
            ("dyn", "as_str", lkrt_dyn_as_str, ReadsHost, [DynVal], StrPtr);
            ("dyn", "as_list", lkrt_dyn_as_list, ReadsHost, [DynVal], Ptr);
            ("dyn", "as_bool", lkrt_dyn_as_bool, ReadsHost, [DynVal], I64);
            ("dyn", "add", lkrt_dyn_add, WritesHost, [DynVal, DynVal], DynVal);
            ("dyn", "sub", lkrt_dyn_sub, ReadsHost, [DynVal, DynVal], DynVal);
            ("dyn", "mul", lkrt_dyn_mul, ReadsHost, [DynVal, DynVal], DynVal);
            ("dyn", "div", lkrt_dyn_div, ReadsHost, [DynVal, DynVal], DynVal);
            ("dyn", "mod", lkrt_dyn_mod, ReadsHost, [DynVal, DynVal], DynVal);
            ("dyn", "eq", lkrt_dyn_eq, ReadsHost, [DynVal, DynVal], I64);
            ("dyn", "lt", lkrt_dyn_lt, ReadsHost, [DynVal, DynVal], I64);
            ("dyn", "le", lkrt_dyn_le, ReadsHost, [DynVal, DynVal], I64);
            ("dyn", "gt", lkrt_dyn_gt, ReadsHost, [DynVal, DynVal], I64);
            ("dyn", "ge", lkrt_dyn_ge, ReadsHost, [DynVal, DynVal], I64);
            ("dyn", "index", lkrt_dyn_index, ReadsHost, [DynVal, I64], DynVal);
            ("dyn", "from_map", lkrt_dyn_from_map, Pure, [Ptr], DynVal);
            ("dyn", "field", lkrt_dyn_field, ReadsHost, [DynVal, StrPtr], DynVal);
            ("dyn", "len_of", lkrt_dyn_len_of, ReadsHost, [DynVal], I64);
            ("dyn", "display", lkrt_dyn_display, WritesHost, [DynVal], StrPtr);
            ("dyn", "display_quoted", lkrt_dyn_display_quoted, WritesHost, [DynVal], StrPtr);
            ("map_h", "str_dyn_new", lkrt_lkmap_str_dyn_new, WritesHost, [], Ptr);
            ("map_h", "str_dyn_set", lkrt_lkmap_str_dyn_set, WritesHost, [Ptr, StrPtr, DynVal], Nil);
            ("map_h", "str_dyn_get", lkrt_lkmap_str_dyn_get, ReadsHost, [Ptr, StrPtr], DynVal);
            ("map_h", "str_dyn_len", lkrt_lkmap_str_dyn_len, ReadsHost, [Ptr], I64);
            ("map_h", "str_dyn_has", lkrt_lkmap_str_dyn_has, ReadsHost, [Ptr, StrPtr], I64);
            ("list_h", "i64_to_dyn", lkrt_lklist_i64_to_dyn, WritesHost, [Ptr], Ptr);
            ("list_h", "f64_to_dyn", lkrt_lklist_f64_to_dyn, WritesHost, [Ptr], Ptr);
            ("list_h", "str_to_dyn", lkrt_lklist_str_to_dyn, WritesHost, [Ptr], Ptr);
            ("list_h", "dyn_new", lkrt_lklist_dyn_new, WritesHost, [], Ptr);
            ("list_h", "dyn_push", lkrt_lklist_dyn_push, WritesHost, [Ptr, DynVal], Nil);
            ("list_h", "dyn_at", lkrt_lklist_dyn_at, ReadsHost, [Ptr, I64], DynVal);
            ("list_h", "dyn_set", lkrt_lklist_dyn_set, WritesHost, [Ptr, I64, DynVal], Nil);
            ("list_h", "dyn_len", lkrt_lklist_dyn_len, ReadsHost, [Ptr], I64);
            ("list_h", "dyn_eq", lkrt_lklist_dyn_eq, ReadsHost, [Ptr, Ptr], I64);
            ("list_h", "dyn_chunk", lkrt_lklist_dyn_chunk, WritesHost, [Ptr, I64], Ptr);
            ("list_h", "dyn_enumerate", lkrt_lklist_dyn_enumerate, WritesHost, [Ptr], Ptr);
            ("list_h", "dyn_zip", lkrt_lklist_dyn_zip, WritesHost, [Ptr, Ptr], Ptr);
            ("list_h", "dyn_unique", lkrt_lklist_dyn_unique, WritesHost, [Ptr], Ptr);
            ("list_h", "dyn_flatten", lkrt_lklist_dyn_flatten, WritesHost, [Ptr], Ptr);
            ("list_h", "dyn_slice_from", lkrt_lklist_dyn_slice_from, WritesHost, [Ptr, I64], Ptr);
            ("list_h", "dyn_contains", lkrt_lklist_dyn_contains, ReadsHost, [Ptr, DynVal], I64);
            ("list_h", "dyn_display", lkrt_lklist_dyn_display, WritesHost, [Ptr], StrPtr);
            ("arith", "i64_div", lkrt_i64_div_checked, ReadsHost, [I64, I64], I64);
            ("arith", "i64_mod", lkrt_i64_mod_checked, ReadsHost, [I64, I64], I64);
            ("arith", "f64_div", lkrt_f64_div_checked, ReadsHost, [F64, F64], F64);
            ("arith", "f64_mod", lkrt_f64_mod_checked, ReadsHost, [F64, F64], F64);
        }
    };
}

/// Expands the ABI table into the [`ABI_FUNCTIONS`] const slice.
macro_rules! define_abi_functions {
    ($( ($module:literal, $name:literal, $symbol:ident, $effect:ident, [$($param:ident),* $(,)?], $ret:ident) );* $(;)?) => {
        /// The complete native ABI surface. Codegen renders `declare`s from this; `lkrt`
        /// provides one `#[no_mangle]` implementation per `symbol` (checked against this
        /// table by `lkrt`'s conformance test via [`for_each_abi_fn`]).
        pub const ABI_FUNCTIONS: &[AbiFn] = &[
            $( AbiFn {
                module: $module,
                name: $name,
                symbol: stringify!($symbol),
                params: &[$(AbiType::$param),*],
                result: AbiType::$ret,
                effect: AbiEffect::$effect,
            } ),*
        ];
    };
}

for_each_abi_fn!(define_abi_functions);

/// Looks up an ABI function by its `(module, name)` identity.
pub fn find(module: &str, name: &str) -> Option<&'static AbiFn> {
    ABI_FUNCTIONS
        .iter()
        .find(|intrinsic| intrinsic.module == module && intrinsic.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn symbols_are_unique() {
        let mut seen = std::collections::HashSet::new();
        for f in ABI_FUNCTIONS {
            assert!(seen.insert(f.symbol), "duplicate ABI symbol: {}", f.symbol);
        }
    }

    #[test]
    fn find_resolves_known_entry() {
        let f = find("map_h", "str_i64_set").expect("known entry");
        assert_eq!(f.symbol, "lkrt_lkmap_str_i64_set");
        assert_eq!(f.result, AbiType::Nil);
    }
}
