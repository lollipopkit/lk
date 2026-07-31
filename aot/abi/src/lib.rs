//! The ABI table is data, not behaviour, so it builds without std —
//! `lkrt` needs it on bare metal.
#![cfg_attr(not(feature = "std"), no_std)]
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

/// What a call does with the container handle passed as its **receiver**
/// (parameter 0), and whether it returns a fresh one.
///
/// This is the memory-safety input to the scope-drop pass in
/// `lk_aot_mir::opt`, which releases a loop-local container at the end of its
/// block. Releasing a handle the runtime still holds is a use-after-free, so
/// the default is [`Receiver::Retained`]: an ABI entry is un-releasable until
/// someone audits its implementation and says otherwise here.
///
/// The question is deliberately only about parameter 0. A handle appearing in
/// any other argument position is treated as escaping by the pass itself
/// (`list_h.dyn_push(other, handle)` stores it), so entries need not describe
/// what they do with their non-receiver arguments.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Receiver {
    /// Conservative default: the call may store the receiver somewhere that
    /// outlives it (`dyn.from_list` boxes the handle into a value).
    Retained,
    /// The call only reads or mutates the receiver in place; after it returns,
    /// the runtime holds no new reference to it.
    Borrowed,
    /// Borrows the receiver (or takes none) *and* returns a freshly allocated
    /// arena container handle — the constructors the pass looks for.
    Constructs,
    /// Returns a freshly allocated handle that **points back into the
    /// receiver** — `xs.slice(a, b)`, whose window reads through to `xs` on
    /// every access (`lkrt::lkslice`).
    ///
    /// Both halves matter and neither of the other two says both: the result is
    /// releasable at the end of its scope like any other fresh handle, while
    /// the receiver is not, because releasing a list that a live window still
    /// addresses is a use-after-free. Spelling this as `Constructs` would have
    /// freed the source; spelling it `Retained` would have kept every window
    /// alive to process exit.
    ConstructsView,
}

impl Receiver {
    /// Whether the runtime may hold on to the receiver after the call.
    pub fn retains(self) -> bool {
        matches!(self, Receiver::Retained | Receiver::ConstructsView)
    }

    /// Whether the call's result is a fresh arena container handle.
    pub fn constructs(self) -> bool {
        matches!(self, Receiver::Constructs | Receiver::ConstructsView)
    }
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
    /// Handle ownership contract; see [`Receiver`]. Defaults to
    /// [`Receiver::Retained`] for entries that do not state one.
    pub receiver: Receiver,
}

/// Invokes the given callback macro with every ABI table entry, in order. This is
/// An entry with no emitter is not "available", it is **unverified**: nothing
/// exercises its argument marshalling or its receiver class, so the first caller
/// is the one that finds out whether the row is right. Three such rows
/// (`dyn.as_typed_map`, `dyn.as_set`, `dyn.as_bytes`) were added for symmetry
/// with their `from_*` counterparts and deleted unused — add the row with the
/// call site, not before it.
///
/// the single source of truth (RFC aot-redesign §3.3): the [`ABI_FUNCTIONS`] const
/// table below and `lkrt`'s compile-time signature-conformance checks both expand
/// from it, so a signature can no longer drift between the schema, the codegen
/// `declare`s, and the runtime implementation without failing the build/tests.
///
/// Entry shape:
/// `("module", "name", symbol_ident, Effect, [ParamTypes...], RetType);` or, when
/// the entry takes or returns a container handle, with an explicit ownership
/// contract appended:
/// `("module", "name", symbol_ident, Effect, [ParamTypes...], RetType, Receiver);`
/// Omitting it means [`Receiver::Retained`] — the conservative choice, so a new
/// entry can never accidentally become releasable.
#[macro_export]
macro_rules! for_each_abi_fn {
    ($callback:ident) => {
        $callback! {
            // CPU control. All `WritesHost`: a barrier's entire content is
            // its effect on *other* accesses' ordering, so marking one pure
            // would license the optimiser to drop the very thing it is for.
            ("cpu", "barrier", lkrt_cpu_barrier, WritesHost, [], Nil);
            ("cpu", "compiler_barrier", lkrt_cpu_compiler_barrier, WritesHost, [], Nil);
            ("cpu", "irq_save", lkrt_cpu_irq_save, WritesHost, [], I64);
            ("cpu", "irq_restore", lkrt_cpu_irq_restore, WritesHost, [I64], Nil);
            ("cpu", "timestamp", lkrt_cpu_timestamp, WritesHost, [], I64);
            ("cpu", "wait_for_interrupt", lkrt_cpu_wait_for_interrupt, WritesHost, [], Nil);
            // System control: descriptor tables, CR2/CR3, the TLB. x86 only,
            // and `WritesHost` including the reads — CR2 changes behind the
            // code's back on every fault, which is its entire purpose, so two
            // reads of it must not be collapsed into one. See lkrt/src/system.rs.
            ("cpu", "load_idt", lkrt_cpu_load_idt, WritesHost, [I64, I64], Nil);
            ("cpu", "load_gdt", lkrt_cpu_load_gdt, WritesHost, [I64, I64], Nil);
            ("cpu", "reload_segments", lkrt_cpu_reload_segments, WritesHost, [I64, I64], Nil);
            ("cpu", "load_task_register", lkrt_cpu_load_task_register, WritesHost, [I64], Nil);
            ("cpu", "read_cr2", lkrt_cpu_read_cr2, WritesHost, [], I64);
            ("cpu", "read_cr3", lkrt_cpu_read_cr3, WritesHost, [], I64);
            ("cpu", "write_cr3", lkrt_cpu_write_cr3, WritesHost, [I64], Nil);
            ("cpu", "raise_interrupt", lkrt_cpu_raise_interrupt, WritesHost, [I64], Nil);
            ("cpu", "invalidate_page", lkrt_cpu_invalidate_page, WritesHost, [I64], Nil);
            // Volatile MMIO has no entries here any more, and that absence is
            // the point: `volatile_read_uN`/`volatile_write_uN` lower to a real
            // machine load and store (`Inst::VolatileLoad`), not to a call.
            // What made them calls was that Cranelift has no volatile flag and
            // its alias analysis collapses two accesses to one address; what
            // replaced them is a `sequence_point` before each access, which
            // emits nothing and moves the key that analysis works from.
            // Port I/O — `WritesHost` for the same reason the MMIO reads are:
            // reading a device port can change its state, so it must not be
            // collapsed with another read of the same port.
            ("port", "in_u8", lkrt_port_in_u8, WritesHost, [I64], I64);
            ("port", "in_u16", lkrt_port_in_u16, WritesHost, [I64], I64);
            ("port", "in_u32", lkrt_port_in_u32, WritesHost, [I64], I64);
            ("port", "out_u8", lkrt_port_out_u8, WritesHost, [I64, I64], Nil);
            ("port", "out_u16", lkrt_port_out_u16, WritesHost, [I64, I64], Nil);
            ("port", "out_u32", lkrt_port_out_u32, WritesHost, [I64, I64], Nil);
            ("lkrt", "abi_version", lkrt_abi_version, Pure, [], I64);
            ("lkrt", "rt_begin", lkrt_rt_begin, WritesHost, [I64], Nil);
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
            // Native protected calls (`try$call`, plan G): handler-stack
            // frames around a `_setjmp` in the generated code, the raised
            // value, and the raise entry points (no live handler → the
            // existing loud abort). Cells are the VM's `UpvalCell` — shared
            // mutable boxes for captures assigned inside a closure.
            ("rt", "try_push", lkrt_rt_try_push, WritesHost, [], Ptr);
            ("rt", "try_pop", lkrt_rt_try_pop, WritesHost, [], Nil);
            ("rt", "current_error", lkrt_rt_current_error, ReadsHost, [], DynVal);
            ("rt", "raise_dyn", lkrt_rt_raise_dyn, WritesHost, [DynVal], Nil);
            ("rt", "raise_msg", lkrt_rt_raise_msg, WritesHost, [StrPtr], Nil);
            ("rt", "cell_new", lkrt_rt_cell_new, WritesHost, [DynVal], Ptr);
            // The raw-handle family: a typed container parked as-is, because
            // boxing one is an element-wise copy. Tag-checked at both ends.
            ("rt", "cell_new_raw", lkrt_rt_cell_new_raw, WritesHost, [I64], Ptr);
            ("rt", "cell_get_raw", lkrt_rt_cell_get_raw, ReadsHost, [Ptr], I64);
            ("rt", "cell_set_raw", lkrt_rt_cell_set_raw, WritesHost, [Ptr, I64], Nil);
            ("rt", "cell_get", lkrt_rt_cell_get, ReadsHost, [Ptr], DynVal);
            ("rt", "cell_set", lkrt_rt_cell_set, WritesHost, [Ptr, DynVal], Nil);
            // Early release of an arena container proven dead (scope drop).
            ("rt", "handle_release", lkrt_rt_handle_release, WritesHost, [Ptr], Nil);
            // Same, plus the arena strings the container itself created — only
            // emitted when the pass has proven no element ever left it
            // (`lk_aot_mir::opt`).
            ("rt", "handle_release_deep", lkrt_rt_handle_release_deep, WritesHost, [Ptr], Nil);
            // Native channels + goroutine threads (plan H: OS threads +
            // deep-copy isolate channels; ids are i64). Blocking send/recv,
            // Go close semantics (buffer drains, then raises), snapshot
            // argument blocks for spawn, join-once task await.
            ("chan", "new", lkrt_chan_new, WritesHost, [I64], I64);
            ("time", "timeout", lkrt_time_timeout, WritesHost, [I64], I64);
            ("time", "after", lkrt_time_after, WritesHost, [I64], I64);
            ("chan", "send", lkrt_chan_send, WritesHost, [I64, DynVal], Nil);
            ("chan", "recv", lkrt_chan_recv, WritesHost, [I64], DynVal);
            ("chan", "close", lkrt_chan_close, WritesHost, [I64], Nil);
            ("chan", "try_send", lkrt_chan_try_send, WritesHost, [I64, DynVal], I64);
            ("chan", "try_recv", lkrt_chan_try_recv, WritesHost, [I64], DynVal);
            ("chan", "len", lkrt_chan_len, ReadsHost, [I64], I64);
            ("chan", "capacity", lkrt_chan_capacity, ReadsHost, [I64], I64);
            ("chan", "is_closed", lkrt_chan_is_closed, ReadsHost, [I64], I64);
            ("chan", "select", lkrt_chan_select, WritesHost, [Ptr, Ptr, Ptr, Ptr, I64], Ptr);
            // `encoding` submodules: the VM's exact crates + conversion rules
            // (`core/src/val/de.rs`); object key order mirrors two-stage.
            ("json", "parse", lkrt_json_parse, WritesHost, [StrPtr], DynVal);
            ("json", "stringify", lkrt_json_stringify, WritesHost, [DynVal], StrPtr);
            ("yaml", "parse", lkrt_yaml_parse, WritesHost, [StrPtr], DynVal);
            ("yaml", "stringify", lkrt_yaml_stringify, WritesHost, [DynVal], StrPtr);
            ("toml", "parse", lkrt_toml_parse, WritesHost, [StrPtr], DynVal);
            ("toml", "stringify", lkrt_toml_stringify, WritesHost, [DynVal], StrPtr);
            // `base64`/`hex`/`url`: the same crates the stdlib module uses, so
            // the text is byte-identical. `WritesHost` like every other
            // arena-allocating string producer. `url.decode_component` raises on
            // a malformed escape.
            // `Bytes` handles: an arena-owned `Vec<u8>`, the same shape a list
            // handle has. Content equality and `Bytes([…])` display, both the
            // VM's rules.
            ("bytes_h", "from_str", lkrt_lkbytes_from_str, WritesHost, [StrPtr], Ptr);
            ("bytes_h", "len", lkrt_lkbytes_len, Pure, [Ptr], I64);
            ("bytes_h", "is_empty", lkrt_lkbytes_is_empty, Pure, [Ptr], I64);
            ("bytes_h", "eq", lkrt_lkbytes_eq, Pure, [Ptr, Ptr], I64);
            ("bytes_h", "get", lkrt_lkbytes_get, Pure, [Ptr, I64], DynVal);
            ("bytes_h", "concat", lkrt_lkbytes_concat, WritesHost, [Ptr, Ptr], Ptr);
            ("bytes_h", "slice", lkrt_lkbytes_slice, WritesHost, [Ptr, I64, I64], Ptr);
            // `Bytes` had a carrier and four methods; ten of its fourteen fell
            // back. A count is not a position, so take/skip get their own guard
            // rather than borrowing `slice`'s.
            ("bytes_h", "take", lkrt_lkbytes_take, WritesHost, [Ptr, I64], Ptr);
            ("bytes_h", "skip", lkrt_lkbytes_skip, WritesHost, [Ptr, I64], Ptr);
            ("bytes_h", "index_of", lkrt_lkbytes_index_of, ReadsHost, [Ptr, I64], DynVal);
            ("bytes_h", "contains", lkrt_lkbytes_contains, ReadsHost, [Ptr, I64], I64);
            ("bytes_h", "from_i64_list", lkrt_lkbytes_from_i64_list, WritesHost, [Ptr], Ptr);
            ("bytes_h", "to_i64_list", lkrt_lkbytes_to_i64_list, WritesHost, [Ptr], Ptr);
            // The three reductions. `min`/`max` answer nil on an empty
            // sequence, so they box; `sum` answers `0` and does not.
            ("bytes_h", "sum", lkrt_lkbytes_sum, ReadsHost, [Ptr], I64);
            ("bytes_h", "min", lkrt_lkbytes_min, ReadsHost, [Ptr], DynVal);
            ("bytes_h", "max", lkrt_lkbytes_max, ReadsHost, [Ptr], DynVal);
            ("list_h", "i64_sum", lkrt_lklist_i64_sum, ReadsHost, [Ptr], I64);
            ("list_h", "f64_sum", lkrt_lklist_f64_sum, ReadsHost, [Ptr], F64);
            ("list_h", "i64_min", lkrt_lklist_i64_min, ReadsHost, [Ptr], DynVal);
            ("list_h", "i64_max", lkrt_lklist_i64_max, ReadsHost, [Ptr], DynVal);
            ("list_h", "f64_min", lkrt_lklist_f64_min, ReadsHost, [Ptr], DynVal);
            ("list_h", "f64_max", lkrt_lklist_f64_max, ReadsHost, [Ptr], DynVal);
            ("list_h", "str_min", lkrt_lklist_str_min, ReadsHost, [Ptr], DynVal);
            ("list_h", "str_max", lkrt_lklist_str_max, ReadsHost, [Ptr], DynVal);
            ("bytes_h", "utf8", lkrt_lkbytes_utf8, WritesHost, [Ptr], StrPtr);
            ("bytes_h", "utf8_lossy", lkrt_lkbytes_utf8_lossy, WritesHost, [Ptr], StrPtr);
            ("bytes_h", "to_str", lkrt_lkbytes_to_str, WritesHost, [Ptr], StrPtr);
            ("base64", "decode", lkrt_base64_decode, WritesHost, [StrPtr], Ptr);
            ("hex", "decode", lkrt_hex_decode, WritesHost, [StrPtr], Ptr);
            ("base64", "encode", lkrt_base64_encode, WritesHost, [StrPtr], StrPtr);
            ("hex", "encode", lkrt_hex_encode, WritesHost, [StrPtr], StrPtr);
            // `uuid.v4` is deliberately not `Pure`: two calls are two UUIDs, and
            // CSE merges equal `Pure` calls in a dominance scope.
            // `regex` compiles through a shared bounded cache, so a call is
            // `ReadsHost`, not `Pure` — two identical calls are still cheap, but
            // the cache is process state.
            ("regex", "is_match", lkrt_regex_is_match, ReadsHost, [StrPtr, StrPtr], I64);
            ("regex", "split", lkrt_regex_split, WritesHost, [StrPtr, StrPtr], Ptr);
            ("regex", "find", lkrt_regex_find, WritesHost, [StrPtr, StrPtr], DynVal);
            ("regex", "find_all", lkrt_regex_find_all, WritesHost, [StrPtr, StrPtr], Ptr);
            ("regex", "captures", lkrt_regex_captures, WritesHost, [StrPtr, StrPtr], DynVal);
            ("regex", "replace", lkrt_regex_replace, WritesHost, [StrPtr, StrPtr, StrPtr], StrPtr);
            // `random`: nondeterministic to a value, so never `Pure` (CSE would
            // merge two rolls into one).
            ("process", "id", lkrt_process_id, ReadsHost, [], I64);
            ("process", "set_cwd", lkrt_process_set_cwd, WritesHost, [StrPtr], I64);
            ("process", "exit", lkrt_process_exit, WritesHost, [I64], Nil);
            ("process", "status", lkrt_process_status, WritesHost, [StrPtr, Ptr], I64);
            ("process", "output_string", lkrt_process_output_string, WritesHost, [StrPtr, Ptr], StrPtr);
            ("process", "output", lkrt_process_output, WritesHost, [StrPtr, Ptr], Ptr);
            ("process", "status_noargs", lkrt_process_status_noargs, WritesHost, [StrPtr], I64);
            ("process", "output_string_noargs", lkrt_process_output_string_noargs, WritesHost, [StrPtr], StrPtr);
            ("process", "output_noargs", lkrt_process_output_noargs, WritesHost, [StrPtr], Ptr);
            ("random", "int", lkrt_random_int, WritesHost, [I64, I64], I64);
            ("random", "float", lkrt_random_float, WritesHost, [], F64);
            ("random", "bool", lkrt_random_bool, WritesHost, [], I64);
            ("random", "bool_p", lkrt_random_bool_p, WritesHost, [F64], I64);
            ("random", "bytes", lkrt_random_bytes, WritesHost, [I64], Ptr);
            ("random", "choice_i64", lkrt_random_choice_i64, WritesHost, [Ptr], DynVal);
            ("random", "choice_f64", lkrt_random_choice_f64, WritesHost, [Ptr], DynVal);
            ("random", "choice_str", lkrt_random_choice_str, WritesHost, [Ptr], DynVal);
            ("random", "choice_dyn", lkrt_random_choice_dyn, WritesHost, [Ptr], DynVal);
            ("random", "shuffle_i64", lkrt_random_shuffle_i64, WritesHost, [Ptr], Ptr);
            ("random", "shuffle_f64", lkrt_random_shuffle_f64, WritesHost, [Ptr], Ptr);
            ("random", "shuffle_str", lkrt_random_shuffle_str, WritesHost, [Ptr], Ptr);
            ("random", "shuffle_dyn", lkrt_random_shuffle_dyn, WritesHost, [Ptr], Ptr);
            ("uuid", "v4", lkrt_uuid_v4, WritesHost, [], StrPtr);
            ("uuid", "parse", lkrt_uuid_parse, WritesHost, [StrPtr], StrPtr);
            ("uuid", "is_valid", lkrt_uuid_is_valid, Pure, [StrPtr], I64);
            ("base64", "encode_bytes", lkrt_base64_encode_bytes, WritesHost, [Ptr], StrPtr);
            ("hex", "encode_bytes", lkrt_hex_encode_bytes, WritesHost, [Ptr], StrPtr);
            // `hash`, both carriers of each member (`Bytes | String`).
            ("hash", "sha256_str", lkrt_hash_sha256_str, Pure, [StrPtr], StrPtr);
            ("hash", "sha1_str", lkrt_hash_sha1_str, Pure, [StrPtr], StrPtr);
            ("hash", "crc32_str", lkrt_hash_crc32_str, Pure, [StrPtr], I64);
            ("hash", "fnv64_str", lkrt_hash_fnv64_str, Pure, [StrPtr], I64);
            ("hash", "sha256_bytes", lkrt_hash_sha256_bytes, ReadsHost, [Ptr], StrPtr);
            ("hash", "sha1_bytes", lkrt_hash_sha1_bytes, ReadsHost, [Ptr], StrPtr);
            ("hash", "crc32_bytes", lkrt_hash_crc32_bytes, ReadsHost, [Ptr], I64);
            ("hash", "fnv64_bytes", lkrt_hash_fnv64_bytes, ReadsHost, [Ptr], I64);
            ("url", "encode_component", lkrt_url_encode_component, WritesHost, [StrPtr], StrPtr);
            ("url", "decode_component", lkrt_url_decode_component, WritesHost, [StrPtr], StrPtr);
            ("rt", "spawn_args_new", lkrt_spawn_args_new, WritesHost, [], Ptr);
            ("rt", "spawn_args_push", lkrt_spawn_args_push, WritesHost, [Ptr, DynVal], Nil);
            ("rt", "spawn_arg", lkrt_spawn_arg, ReadsHost, [Ptr, I64], DynVal);
            ("rt", "spawn0", lkrt_spawn0, WritesHost, [Ptr], I64);
            ("rt", "spawn1", lkrt_spawn1, WritesHost, [Ptr, Ptr], I64);
            ("rt", "spawn2", lkrt_spawn2, WritesHost, [Ptr, Ptr], I64);
            ("rt", "spawn3", lkrt_spawn3, WritesHost, [Ptr, Ptr], I64);
            ("rt", "spawn4", lkrt_spawn4, WritesHost, [Ptr, Ptr], I64);
            ("rt", "task_await", lkrt_task_await, WritesHost, [I64], DynVal);
            ("socket", "addr", lkrt_socket_addr, Pure, [StrPtr, I64], StrPtr);
            ("tcp", "connect", lkrt_tcp_connect, WritesHost, [StrPtr], I64);
            ("tcp", "read", lkrt_tcp_read, WritesHost, [I64, I64], Ptr);
            ("tcp", "write_str", lkrt_tcp_write_str, WritesHost, [I64, StrPtr], I64);
            ("tcp", "write_bytes", lkrt_tcp_write_bytes, WritesHost, [I64, Ptr], I64);
            ("tcp", "close", lkrt_tcp_close, WritesHost, [I64], I64);
            // Not `Pure`: it `take_bytes` — the handle is *consumed*, so a
            // second call with the same handle fails where the first one
            // succeeded. Mislabeling it would let a CSE pass collapse the two.
            ("lkrt", "handle_close", lkrt_handle_close, WritesHost, [I64], I64);
            ("io.std", "write", lkrt_io_std_write, WritesHost, [I64, StrPtr, I64], I64);
            ("io.std", "flush", lkrt_io_std_flush, WritesHost, [I64], I64);
            ("io.std", "read_to_string", lkrt_io_std_read_to_string, WritesHost, [I64], StrPtr);
            ("env", "get", lkrt_env_get, ReadsHost, [StrPtr, Ptr], I64);
            ("env", "get_or", lkrt_env_get_or, ReadsHost, [StrPtr, StrPtr], StrPtr);
            ("env", "has", lkrt_env_has, ReadsHost, [StrPtr], I64);
            ("fs", "read", lkrt_fs_read, ReadsHost, [StrPtr], Ptr);
            ("fs", "read_to_string", lkrt_fs_read_to_string, ReadsHost, [StrPtr], StrPtr);
            ("fs", "write_str", lkrt_fs_write_str, WritesHost, [StrPtr, StrPtr], I64);
            ("fs", "write_bytes", lkrt_fs_write_bytes, WritesHost, [StrPtr, Ptr], I64);
            ("fs", "exists", lkrt_fs_exists, ReadsHost, [StrPtr], I64);
            ("fs", "metadata_len", lkrt_fs_metadata_len, ReadsHost, [StrPtr], I64);
            ("fs", "metadata_is_file", lkrt_fs_metadata_is_file, ReadsHost, [StrPtr], I64);
            ("fs", "metadata_is_dir", lkrt_fs_metadata_is_dir, ReadsHost, [StrPtr], I64);
            ("fs", "metadata_readonly", lkrt_fs_metadata_readonly, ReadsHost, [StrPtr], I64);
            ("fs", "canonicalize", lkrt_fs_canonicalize, ReadsHost, [StrPtr], DynVal);
            ("fs", "metadata_map", lkrt_fs_metadata_map, WritesHost, [StrPtr], Ptr);
            ("env", "vars_map", lkrt_env_vars_map, WritesHost, [], Ptr);
            ("fs", "is_file", lkrt_fs_is_file, ReadsHost, [StrPtr], I64);
            ("fs", "is_dir", lkrt_fs_is_dir, ReadsHost, [StrPtr], I64);
            ("fs", "append_str", lkrt_fs_append_str, WritesHost, [StrPtr, StrPtr], I64);
            ("fs", "append_bytes", lkrt_fs_append_bytes, WritesHost, [StrPtr, Ptr], I64);
            ("fs", "create_dir", lkrt_fs_create_dir, WritesHost, [StrPtr], I64);
            ("fs", "create_dir_all", lkrt_fs_create_dir_all, WritesHost, [StrPtr], I64);
            ("fs", "remove_file", lkrt_fs_remove_file, WritesHost, [StrPtr], I64);
            ("fs", "remove_dir", lkrt_fs_remove_dir, WritesHost, [StrPtr], I64);
            ("fs", "remove_dir_all", lkrt_fs_remove_dir_all, WritesHost, [StrPtr], I64);
            ("fs", "rename", lkrt_fs_rename, WritesHost, [StrPtr, StrPtr], I64);
            ("fs", "copy", lkrt_fs_copy, WritesHost, [StrPtr, StrPtr], I64);
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
            // `sin`/`cos` were native and `tan` was not; the inverse and log
            // families were absent entirely. Their domain guards raise the
            // stdlib module's own words, because a caught error's text is the
            // program's output.
            ("math", "tan", lkrt_math_tan, Pure, [F64], F64);
            ("math", "asin", lkrt_math_asin, Pure, [F64], F64);
            ("math", "acos", lkrt_math_acos, Pure, [F64], F64);
            ("math", "atan", lkrt_math_atan, Pure, [F64], F64);
            ("math", "atan2", lkrt_math_atan2, Pure, [F64, F64], F64);
            ("math", "log", lkrt_math_log, Pure, [F64], F64);
            ("math", "log10", lkrt_math_log10, Pure, [F64], F64);
            ("math", "log2", lkrt_math_log2, Pure, [F64], F64);
            ("math", "clamp_i64", lkrt_math_clamp_i64, Pure, [I64, I64, I64], I64);
            ("math", "exp", lkrt_math_exp, Pure, [F64], F64);
            ("math", "pow", lkrt_math_pow, Pure, [F64, F64], F64);
            ("math", "hypot", lkrt_math_hypot, Pure, [F64, F64], F64);
            ("math", "cbrt", lkrt_math_cbrt, Pure, [F64], F64);
            ("math", "is_nan", lkrt_math_is_nan, Pure, [F64], I64);
            // `math.sign` keeps its argument's numeric flavor (Int → signum,
            // Float → ±1.0/0.0); the lowering dispatches on the static type.
            ("math", "sign_i64", lkrt_math_sign_i64, Pure, [I64], I64);
            ("math", "sign_f64", lkrt_math_sign_f64, Pure, [F64], F64);
            // The `path` module's fixed-arity members. `String?` results arrive
            // boxed, the same convention `string.strip_prefix` uses.
            ("path", "parent", lkrt_path_parent, Pure, [StrPtr], DynVal);
            ("path", "file_name", lkrt_path_file_name, Pure, [StrPtr], DynVal);
            ("path", "file_stem", lkrt_path_file_stem, Pure, [StrPtr], DynVal);
            ("path", "extension", lkrt_path_extension, Pure, [StrPtr], DynVal);
            ("path", "with_extension", lkrt_path_with_extension, WritesHost, [StrPtr, StrPtr], StrPtr);
            ("path", "is_absolute", lkrt_path_is_absolute, Pure, [StrPtr], I64);
            ("path", "components", lkrt_path_components, WritesHost, [StrPtr], Ptr);
            ("path", "sep", lkrt_path_sep, ReadsHost, [], StrPtr);
            ("path", "delimiter", lkrt_path_delimiter, ReadsHost, [], StrPtr);
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
            ("list_h", "i64_new", lkrt_lklist_i64_new, WritesHost, [], Ptr, Constructs);
            ("list_h", "i64_from_range", lkrt_lklist_i64_from_range, WritesHost, [I64, I64, I64, I64], Ptr, Constructs);
            ("list_h", "i64_take", lkrt_lklist_i64_take, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "i64_skip", lkrt_lklist_i64_skip, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "f64_take", lkrt_lklist_f64_take, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "f64_skip", lkrt_lklist_f64_skip, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "str_take", lkrt_lklist_str_take, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "str_skip", lkrt_lklist_str_skip, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "i64_chain", lkrt_lklist_i64_chain, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "f64_chain", lkrt_lklist_f64_chain, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "str_chain", lkrt_lklist_str_chain, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "i64_push", lkrt_lklist_i64_push, WritesHost, [Ptr, I64], Nil, Borrowed);
            // `clear()` on every carrier: the operation does not depend on the
            // element type, so all four rows land together.
            ("list_h", "i64_clear", lkrt_lklist_i64_clear, WritesHost, [Ptr], Nil, Borrowed);
            // `pop` / `insert` / `remove_at`: none of the three had a lowering on
            // any carrier, so a single `xs.pop()` dropped its whole module to the
            // VM. `drop_last` is `pop`'s mutation half — the read reuses the
            // carrier's `Maybe` machinery (see `list_drop_last!` for why a
            // `*_pop` returning `Maybe<f64>` by value is not portable). `insert`
            // answers nothing for the same reason `clear` does: the VM evaluates
            // it to the receiver, which the lowering already holds, and a
            // `Borrowed` pointer return would hand back an unowned handle.
            ("list_h", "i64_drop_last", lkrt_lklist_i64_drop_last, WritesHost, [Ptr], Nil, Borrowed);
            ("list_h", "f64_drop_last", lkrt_lklist_f64_drop_last, WritesHost, [Ptr], Nil, Borrowed);
            ("list_h", "str_drop_last", lkrt_lklist_str_drop_last, WritesHost, [Ptr], Nil, Borrowed);
            ("list_h", "i64_insert", lkrt_lklist_i64_insert, WritesHost, [Ptr, I64, I64], Nil, Borrowed);
            ("list_h", "f64_insert", lkrt_lklist_f64_insert, WritesHost, [Ptr, I64, F64], Nil, Borrowed);
            ("list_h", "str_insert", lkrt_lklist_str_insert, WritesHost, [Ptr, I64, StrPtr], Nil, Borrowed);
            ("list_h", "i64_remove_at", lkrt_lklist_i64_remove_at, WritesHost, [Ptr, I64], I64, Borrowed);
            ("list_h", "f64_remove_at", lkrt_lklist_f64_remove_at, WritesHost, [Ptr, I64], F64, Borrowed);
            ("list_h", "str_remove_at", lkrt_lklist_str_remove_at, WritesHost, [Ptr, I64], StrPtr, Borrowed);
            ("list_h", "f64_clear", lkrt_lklist_f64_clear, WritesHost, [Ptr], Nil, Borrowed);
            ("list_h", "str_clear", lkrt_lklist_str_clear, WritesHost, [Ptr], Nil, Borrowed);
            ("list_h", "dyn_clear", lkrt_lklist_dyn_clear, WritesHost, [Ptr], Nil, Borrowed);
            // List HOF over compiled zero-capture lambdas (`ptr @lk_fn_N`
            // callbacks). The callback may abort (div/0 inside the lambda), so
            // none of these are Pure.
            // VM-exact list display text (`[1,2,3]`), arena-owned.
            ("list_h", "i64_display", lkrt_lklist_i64_display, WritesHost, [Ptr], StrPtr, Borrowed);
            ("list_h", "f64_display", lkrt_lklist_f64_display, WritesHost, [Ptr], StrPtr, Borrowed);
            ("list_h", "str_display", lkrt_lklist_str_display, WritesHost, [Ptr], StrPtr, Borrowed);
            // Structural equality (1/0): same length + element-wise `==`;
            // `i64_f64_eq` compares Int against Float lists with numeric
            // coercion (`[1] == [1.0]` is true in the VM).
            ("list_h", "i64_eq", lkrt_lklist_i64_eq, ReadsHost, [Ptr, Ptr], I64, Borrowed);
            ("list_h", "f64_eq", lkrt_lklist_f64_eq, ReadsHost, [Ptr, Ptr], I64, Borrowed);
            ("list_h", "i64_f64_eq", lkrt_lklist_i64_f64_eq, ReadsHost, [Ptr, Ptr], I64, Borrowed);
            ("list_h", "str_eq", lkrt_lklist_str_eq, ReadsHost, [Ptr, Ptr], I64, Borrowed);
            ("list_h", "i64_map_fn", lkrt_lklist_i64_map_fn, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "i64_filter_fn", lkrt_lklist_i64_filter_fn, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "i64_reduce_fn", lkrt_lklist_i64_reduce_fn, WritesHost, [Ptr, I64, Ptr], I64, Borrowed);
            ("list_h", "i64_len", lkrt_lklist_i64_len, ReadsHost, [Ptr], I64, Borrowed);
            ("list_h", "i64_get", lkrt_lklist_i64_get, ReadsHost, [Ptr, I64, Ptr], I64, Borrowed);
            ("list_h", "i64_at", lkrt_lklist_i64_at, ReadsHost, [Ptr, I64], I64, Borrowed);
            // Store `list[index] = value`; aborts on an out-of-range/negative index
            // (matching the VM's fatal store-index error — a halt, not a nil).
            ("list_h", "i64_set", lkrt_lklist_i64_set, WritesHost, [Ptr, I64, I64], Nil, Borrowed);
            // Linear membership test; returns 0/1 (the caller narrows to `i1`).
            ("list_h", "i64_contains", lkrt_lklist_i64_contains, ReadsHost, [Ptr, I64], I64, Borrowed);
            // Cross-type numeric membership: `1 in [1.0]` and `1.0 in [1, 2]`
            // follow `==`, not the list's internal representation.
            ("list_h", "i64_contains_f64", lkrt_lklist_i64_contains_f64, ReadsHost, [Ptr, F64], I64, Borrowed);
            ("list_h", "f64_contains_i64", lkrt_lklist_f64_contains_i64, ReadsHost, [Ptr, I64], I64, Borrowed);
            // `xs[start..]`: a fresh handle with the elements from `start` on
            // (negative `start` aborts, matching the VM's fatal slice error).
            ("list_h", "i64_slice_from", lkrt_lklist_i64_slice_from, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "f64_slice_from", lkrt_lklist_f64_slice_from, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "str_slice_from", lkrt_lklist_str_slice_from, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "f64_new", lkrt_lklist_f64_new, WritesHost, [], Ptr, Constructs);
            ("list_h", "f64_push", lkrt_lklist_f64_push, WritesHost, [Ptr, F64], Nil, Borrowed);
            ("list_h", "f64_len", lkrt_lklist_f64_len, ReadsHost, [Ptr], I64, Borrowed);
            ("list_h", "f64_at", lkrt_lklist_f64_at, ReadsHost, [Ptr, I64], F64, Borrowed);
            ("list_h", "f64_set", lkrt_lklist_f64_set, WritesHost, [Ptr, I64, F64], Nil, Borrowed);
            // `str_set` completes the carrier set: `xs[i] = v` lowered on `Int`
            // and `Float` only, so the same two lines stayed native or did not
            // depending on the list's representation. (`dyn_set` was already
            // declared further down — it had a row and no lowering using it.)
            ("list_h", "str_set", lkrt_lklist_str_set, WritesHost, [Ptr, I64, StrPtr], Nil, Borrowed);
            ("list_h", "f64_contains", lkrt_lklist_f64_contains, ReadsHost, [Ptr, F64], I64, Borrowed);
            // String-element list handle (elements are interned string-constant pointers).
            ("list_h", "str_new", lkrt_lklist_str_new, WritesHost, [], Ptr, Constructs);
            ("list_h", "str_push", lkrt_lklist_str_push, WritesHost, [Ptr, StrPtr], Nil, Borrowed);
            ("list_h", "str_len", lkrt_lklist_str_len, ReadsHost, [Ptr], I64, Borrowed);
            ("list_h", "str_at", lkrt_lklist_str_at, ReadsHost, [Ptr, I64], StrPtr, Borrowed);
            ("list_h", "str_join", lkrt_lklist_str_join, WritesHost, [Ptr, StrPtr], StrPtr, Borrowed);
            // `join` on the numeric carriers. It was absent because the VM
            // refused a non-string list — one arbitrary rule reproduced as a
            // second one here. The VM renders every element now, and these
            // render them the same way the display helpers do.
            ("list_h", "i64_join", lkrt_lklist_i64_join, WritesHost, [Ptr, StrPtr], StrPtr, Borrowed);
            ("list_h", "f64_join", lkrt_lklist_f64_join, WritesHost, [Ptr, StrPtr], StrPtr, Borrowed);
            ("list_h", "dyn_join", lkrt_lklist_dyn_join, WritesHost, [Ptr, StrPtr], StrPtr, Borrowed);
            // `index_of` is on every sequence in the VM; the lowering had it
            // only on `Str`.
            ("list_h", "i64_index_of", lkrt_lklist_i64_index_of, ReadsHost, [Ptr, I64], DynVal, Borrowed);
            ("list_h", "f64_index_of", lkrt_lklist_f64_index_of, ReadsHost, [Ptr, F64], DynVal, Borrowed);
            ("list_h", "str_index_of", lkrt_lklist_str_index_of, ReadsHost, [Ptr, StrPtr], DynVal, Borrowed);
            ("list_h", "dyn_index_of", lkrt_lklist_dyn_index_of, ReadsHost, [Ptr, DynVal], DynVal, Borrowed);
            ("list_h", "str_contains", lkrt_lklist_str_contains, ReadsHost, [Ptr, StrPtr], I64, Borrowed);
            ("list_h", "i64_slice", lkrt_lklist_i64_slice, WritesHost, [Ptr, I64, I64], Ptr, Constructs);
            // The other carriers, sharing `slice_bounds` with the one above:
            // two-argument `slice` lowered only on `Int`, so `xs.slice(1, 3)`
            // dropped a module to the VM for a reason no program can see.
            ("list_h", "f64_slice", lkrt_lklist_f64_slice, WritesHost, [Ptr, I64, I64], Ptr, Constructs);
            ("list_h", "str_slice", lkrt_lklist_str_slice, WritesHost, [Ptr, I64, I64], Ptr, Constructs);
            ("list_h", "dyn_slice", lkrt_lklist_dyn_slice, WritesHost, [Ptr, I64, I64], Ptr, Constructs);
            // `.slice(start[, end])` is a **window**, not a copy — see the
            // `slice_h` block below. (`i64_slice` above stays a copy: `xs[1..5]`
            // is a range index, which the VM materializes.)
            ("list_h", "i64_sort", lkrt_lklist_i64_sort, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "f64_sort", lkrt_lklist_f64_sort, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "str_sort", lkrt_lklist_str_sort, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "i64_reverse", lkrt_lklist_i64_reverse, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "f64_reverse", lkrt_lklist_f64_reverse, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "str_reverse", lkrt_lklist_str_reverse, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "dyn_reverse", lkrt_lklist_dyn_reverse, WritesHost, [Ptr], Ptr, Constructs);
            // List windows (`lkrt::lkslice`): `xs.slice(a, b)` reads through to
            // `xs` instead of copying it, matching `HeapValue::Slice` in the VM.
            // `ConstructsView` is what keeps the source alive for as long as the
            // window can address it. `get_pair` (by-value `Maybe<i64>`) is
            // declared in codegen, like the list and map variants.
            ("slice_h", "i64_new", lkrt_lkslice_i64_new, WritesHost, [Ptr, I64, I64], Ptr, ConstructsView);
            ("slice_h", "i64_sub", lkrt_lkslice_i64_sub, WritesHost, [Ptr, I64, I64], Ptr, ConstructsView);
            ("slice_h", "i64_len", lkrt_lkslice_i64_len, ReadsHost, [Ptr], I64, Borrowed);
            ("slice_h", "i64_is_empty", lkrt_lkslice_i64_is_empty, ReadsHost, [Ptr], I64, Borrowed);
            // The copy, asked for by name. Its result windows nothing, so it is
            // an ordinary `Constructs`.
            ("slice_h", "i64_to_list", lkrt_lkslice_i64_to_list, WritesHost, [Ptr], Ptr, Constructs);
            // Reads *through* the window rather than materializing it: a
            // window exists so that asking for a sum does not build a list.
            ("slice_h", "i64_sum", lkrt_lkslice_i64_sum, ReadsHost, [Ptr], I64, Borrowed);
            ("slice_h", "i64_min", lkrt_lkslice_i64_min, ReadsHost, [Ptr], DynVal, Borrowed);
            ("slice_h", "i64_max", lkrt_lkslice_i64_max, ReadsHost, [Ptr], DynVal, Borrowed);
            ("slice_h", "i64_contains", lkrt_lkslice_i64_contains, ReadsHost, [Ptr, I64], I64, Borrowed);
            ("slice_h", "i64_index_of", lkrt_lkslice_i64_index_of, ReadsHost, [Ptr, I64], DynVal, Borrowed);
            // Sub-windows, and `WritesHost` because a negative count raises.
            ("slice_h", "i64_take", lkrt_lkslice_i64_take, WritesHost, [Ptr, I64], Ptr, ConstructsView);
            ("slice_h", "i64_skip", lkrt_lkslice_i64_skip, WritesHost, [Ptr, I64], Ptr, ConstructsView);
            ("slice_h", "i64_display", lkrt_lkslice_i64_display, WritesHost, [Ptr], StrPtr, Borrowed);
            // String-keyed map handle. `get_pair` (returning a by-value `Maybe<i64>`) is
            // declared directly in codegen, like the list variant.
            ("map_h", "str_i64_new", lkrt_lkmap_str_i64_new, WritesHost, [], Ptr, Constructs);
            ("map_h", "str_i64_set", lkrt_lkmap_str_i64_set, WritesHost, [Ptr, StrPtr, I64], Nil, Borrowed);
            ("map_h", "str_i64_len", lkrt_lkmap_str_i64_len, ReadsHost, [Ptr], I64, Borrowed);
            // Typed-map display. The order is the carrier's own iteration order,
            // which `vm_mirror` pins to the VM's.
            ("map_h", "str_i64_display", lkrt_lkmap_str_i64_display, WritesHost, [Ptr], StrPtr, Borrowed);
            ("map_h", "str_f64_display", lkrt_lkmap_str_f64_display, WritesHost, [Ptr], StrPtr, Borrowed);
            ("map_h", "str_bool_display", lkrt_lkmap_str_bool_display, WritesHost, [Ptr], StrPtr, Borrowed);
            ("map_h", "i64_i64_display", lkrt_lkmap_i64_i64_display, WritesHost, [Ptr], StrPtr, Borrowed);
            ("map_h", "i64_i64_iter_pairs", lkrt_lkmap_i64_i64_iter_pairs, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "i64_f64_iter_pairs", lkrt_lkmap_i64_f64_iter_pairs, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "i64_f64_display", lkrt_lkmap_i64_f64_display, WritesHost, [Ptr], StrPtr, Borrowed);
            // `{ ..rest }`: a fresh handle with one key removed (chained per key).
            ("map_h", "str_i64_without", lkrt_lkmap_str_i64_without, WritesHost, [Ptr, StrPtr], Ptr, Constructs);
            ("map_h", "str_f64_without", lkrt_lkmap_str_f64_without, WritesHost, [Ptr, StrPtr], Ptr, Constructs);
            // Int-keyed map handle. `get_pair` (by-value `Maybe<i64>`) is declared in codegen.
            ("map_h", "i64_i64_new", lkrt_lkmap_i64_i64_new, WritesHost, [], Ptr, Constructs);
            ("map_h", "i64_i64_set", lkrt_lkmap_i64_i64_set, WritesHost, [Ptr, I64, I64], Nil, Borrowed);
            ("map_h", "i64_i64_len", lkrt_lkmap_i64_i64_len, ReadsHost, [Ptr], I64, Borrowed);
            // String-keyed, f64-valued map. `get_pair` (by-value `Maybe<f64>`) → codegen.
            ("map_h", "str_f64_new", lkrt_lkmap_str_f64_new, WritesHost, [], Ptr, Constructs);
            ("map_h", "str_f64_set", lkrt_lkmap_str_f64_set, WritesHost, [Ptr, StrPtr, F64], Nil, Borrowed);
            ("map_h", "str_f64_len", lkrt_lkmap_str_f64_len, ReadsHost, [Ptr], I64, Borrowed);
            // Int-keyed, f64-valued map. `get_pair` (by-value `Maybe<f64>`) → codegen.
            // Composite string-int key store (`m["n${i}"] = v`): the key is built
            // on the stack inside lkrt, so the store allocates nothing on updates.
            ("map_h", "str_i64_set_ik", lkrt_lkmap_str_i64_set_ik, WritesHost, [Ptr, StrPtr, I64, I64], Nil, Borrowed);
            ("map_h", "str_f64_set_ik", lkrt_lkmap_str_f64_set_ik, WritesHost, [Ptr, StrPtr, I64, F64], Nil, Borrowed);
            ("map_h", "i64_f64_new", lkrt_lkmap_i64_f64_new, WritesHost, [], Ptr, Constructs);
            ("map_h", "i64_f64_set", lkrt_lkmap_i64_f64_set, WritesHost, [Ptr, I64, F64], Nil, Borrowed);
            ("map_h", "i64_f64_len", lkrt_lkmap_i64_f64_len, ReadsHost, [Ptr], I64, Borrowed);
            // Byte-wise string comparison, returning -1/0/1 (the caller compares to 0).
            ("str", "cmp", lkrt_str_cmp, Pure, [StrPtr, StrPtr], I64);
            // `a ++ b` → a freshly allocated C string (`WritesHost`: allocates/leaks).
            ("str", "concat", lkrt_str_concat, WritesHost, [StrPtr, StrPtr], StrPtr);
            // `prefix ++ decimal(suffix)` in one allocation — the composite string-int
            // key shape proven by `GetIndexStrI`/`SetIndexStrI` facts.
            ("str", "concat_i64", lkrt_str_concat_i64, WritesHost, [StrPtr, I64], StrPtr);
            ("str", "char_len", lkrt_str_char_len, Pure, [StrPtr], I64);
            // The *module* `string.len` counts bytes (`str::len`), unlike the
            // `.len()` method's char count.
            ("str", "byte_len", lkrt_str_byte_len, Pure, [StrPtr], I64);
            ("str", "starts_with", lkrt_str_starts_with, Pure, [StrPtr, StrPtr], I64);
            ("str", "contains", lkrt_str_contains, Pure, [StrPtr, StrPtr], I64);
            ("str", "slice_chars", lkrt_str_slice_chars, WritesHost, [StrPtr, I64, I64], StrPtr);
            ("str", "ends_with", lkrt_str_ends_with, Pure, [StrPtr, StrPtr], I64);
            ("str", "lower", lkrt_str_lower, WritesHost, [StrPtr], StrPtr);
            ("str", "upper", lkrt_str_upper, WritesHost, [StrPtr], StrPtr);
            ("str", "trim", lkrt_str_trim, WritesHost, [StrPtr], StrPtr);
            ("str", "index_of", lkrt_str_index_of, WritesHost, [StrPtr, StrPtr], DynVal);
            ("str", "reverse", lkrt_str_reverse, WritesHost, [StrPtr], StrPtr);
            ("str", "repeat", lkrt_str_repeat, WritesHost, [StrPtr, I64], StrPtr);
            ("str", "replace", lkrt_str_replace, WritesHost, [StrPtr, StrPtr, StrPtr], StrPtr);
            ("str", "chars", lkrt_str_chars, WritesHost, [StrPtr], Ptr, Constructs);
            // `string.strip_prefix/suffix` return String-or-nil (boxed Dyn);
            // `count` counts non-overlapping matches, the empty needle included
            // (one between every pair of *characters*, which is what
            // `str::matches` answers); `capitalize`/`title`/`strip`/`pad_*` are
            // Unicode-aware and character-counted, byte-identical to the VM's
            // `core_methods`.
            ("str", "strip_prefix", lkrt_str_strip_prefix, WritesHost, [StrPtr, StrPtr], DynVal);
            ("str", "strip_suffix", lkrt_str_strip_suffix, WritesHost, [StrPtr, StrPtr], DynVal);
            ("str", "strip", lkrt_str_strip, WritesHost, [StrPtr, StrPtr], StrPtr);
            ("str", "pad_left", lkrt_str_pad_left, WritesHost, [StrPtr, I64, StrPtr], StrPtr);
            ("str", "pad_right", lkrt_str_pad_right, WritesHost, [StrPtr, I64, StrPtr], StrPtr);
            // Text → number, the only path there is; the answer is boxed
            // because the module returns `Int?`/`Float?`.
            ("str", "to_int", lkrt_str_to_int, Pure, [StrPtr, I64], DynVal);
            ("str", "to_float", lkrt_str_to_float, Pure, [StrPtr], DynVal);
            ("str", "count", lkrt_str_count, Pure, [StrPtr, StrPtr], I64);
            // Guarded counts: `WritesHost` because a negative one raises, which
            // is an observable effect codegen must not optimize away.
            ("str", "take", lkrt_str_take, WritesHost, [StrPtr, I64], StrPtr);
            ("str", "skip", lkrt_str_skip, WritesHost, [StrPtr, I64], StrPtr);
            ("str", "capitalize", lkrt_str_capitalize, WritesHost, [StrPtr], StrPtr);
            ("str", "title", lkrt_str_title, WritesHost, [StrPtr], StrPtr);
            ("str", "char_at", lkrt_str_char_at, WritesHost, [StrPtr, I64], DynVal);
            // `s.split(sep)` → a fresh `str` list handle (Rust `str::split`, so
            // VM-exact); parts are arena-owned C strings.
            ("str", "split", lkrt_str_split, WritesHost, [StrPtr, StrPtr], Ptr, Constructs);
            // Scalar → display string (the VM's `ToString`), allocating/leaking a C string.
            ("str", "from_i64", lkrt_i64_to_str, WritesHost, [I64], StrPtr);
            // The unsigned reading of the carrier — see `lkrt_u64_to_str`.
            ("str", "from_u64", lkrt_u64_to_str, WritesHost, [I64], StrPtr);
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
            // `!x` on a boxed value: Bool negates, Nil is true, anything else
            // is the VM's loud type error.
            ("dyn", "not", lkrt_dyn_not, ReadsHost, [DynVal], I64);
            // `-x` on a boxed value: Int and Float negate, anything else is
            // the VM's loud type error.
            ("dyn", "neg", lkrt_dyn_neg, ReadsHost, [DynVal], DynVal);
            ("dyn", "as_i64", lkrt_dyn_as_i64, ReadsHost, [DynVal], I64);
            ("dyn", "cast_to_i64", lkrt_dyn_cast_to_i64, ReadsHost, [DynVal], I64);
            ("dyn", "as_f64", lkrt_dyn_as_f64, ReadsHost, [DynVal], F64);
            ("dyn", "as_str", lkrt_dyn_as_str, ReadsHost, [DynVal], StrPtr);
            // Deliberately `Retained`: this returns the *existing* handle held
            // inside the boxed value (`v.payload`), not a fresh one — treating
            // it as a constructor would let the pass free someone else's list.
            ("dyn", "as_list", lkrt_dyn_as_list, ReadsHost, [DynVal], Ptr);
            ("dyn", "as_bool", lkrt_dyn_as_bool, ReadsHost, [DynVal], I64);
            ("dyn", "as_map", lkrt_dyn_as_map, ReadsHost, [DynVal], Ptr);
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
            ("dyn", "get", lkrt_dyn_get, ReadsHost, [DynVal, DynVal], DynVal);
            ("dyn", "from_map", lkrt_dyn_from_map, Pure, [Ptr], DynVal);
            // `Set`/`Bytes` in the boxed universe: without these two tags they
            // could not enter a mixed container, a struct field, or a bridged
            // return at all.
            ("dyn", "from_typed_map", lkrt_dyn_from_typed_map, Pure, [Ptr, I64], DynVal);
            ("dyn", "from_set", lkrt_dyn_from_set, Pure, [Ptr], DynVal);
            ("dyn", "from_bytes", lkrt_dyn_from_bytes, Pure, [Ptr], DynVal);
            ("dyn", "field", lkrt_dyn_field, ReadsHost, [DynVal, StrPtr], DynVal);
            ("dyn", "len_of", lkrt_dyn_len_of, ReadsHost, [DynVal], I64);
            ("dyn", "display", lkrt_dyn_display, WritesHost, [DynVal], StrPtr);
            ("dyn", "display_quoted", lkrt_dyn_display_quoted, WritesHost, [DynVal], StrPtr);
            // Trait-method dispatch marks (plan J1): a struct instance's map
            // handle carries its type id in a side registry (no hidden key);
            // `TraitDispatch` codegen reads the mark, no match raises.
            // Deliberately `Retained` (the default): this records the handle's
            // *address* in the global `OBJ_TYPE_MARKS` table, so releasing a
            // marked map would leave a stale entry that a later allocation at
            // the same address would inherit.
            ("map_h", "obj_mark", lkrt_lkmap_obj_mark, WritesHost, [Ptr, I64], Nil);
            // A struct type's name and field order, described once at startup
            // so `display` can render a marked instance the way the VM does
            // (declaration order, nested values quoted). Two calls rather than
            // a static table: these are shapes the ABI already has.
            ("obj_ty", "begin", lkrt_struct_type_begin, WritesHost, [I64, StrPtr], Nil);
            ("obj_ty", "field", lkrt_struct_type_field, WritesHost, [I64, StrPtr], Nil);
            ("dyn", "obj_type_id", lkrt_dyn_obj_type_id, ReadsHost, [DynVal], I64);
            ("dyn", "method_missing", lkrt_dyn_method_missing, WritesHost, [], Nil);
            ("map_h", "str_dyn_new", lkrt_lkmap_str_dyn_new, WritesHost, [], Ptr, Constructs);
            ("map_h", "str_dyn_set", lkrt_lkmap_str_dyn_set, WritesHost, [Ptr, StrPtr, DynVal], Nil, Borrowed);
            ("map_h", "str_dyn_get", lkrt_lkmap_str_dyn_get, ReadsHost, [Ptr, StrPtr], DynVal, Borrowed);
            ("map_h", "str_dyn_len", lkrt_lkmap_str_dyn_len, ReadsHost, [Ptr], I64, Borrowed);
            ("map_h", "str_dyn_has", lkrt_lkmap_str_dyn_has, ReadsHost, [Ptr, StrPtr], I64, Borrowed);
            ("map_h", "str_dyn_without", lkrt_lkmap_str_dyn_without, WritesHost, [Ptr, StrPtr], Ptr, Constructs);
            // Struct update (`P { ..base, k: v }`): the VM's merge_field_maps
            // two-step insertion + make_struct's fresh field copy.
            ("map_h", "str_dyn_merge", lkrt_lkmap_str_dyn_merge, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("map_h", "str_dyn_merge_typed", lkrt_lkmap_str_dyn_merge_typed, WritesHost, [Ptr, Ptr, I64], Ptr, Constructs);
            ("map_h", "str_dyn_rebuild", lkrt_lkmap_str_dyn_rebuild, WritesHost, [Ptr], Ptr, Constructs);
            // Map-literal protocol (VM-order mirror, plan D1): stage-1 build
            // in source order, then finish into the typed carrier — the
            // result iterates exactly like the VM's two-stage construction.
            ("map_h", "lit_new", lkrt_lkmap_lit_new, WritesHost, [], Ptr, Constructs);
            ("map_h", "lit_set", lkrt_lkmap_lit_set, WritesHost, [Ptr, DynVal, DynVal], Nil, Borrowed);
            ("map_h", "lit_finish_str_i64", lkrt_lkmap_lit_finish_str_i64, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "lit_finish_str_f64", lkrt_lkmap_lit_finish_str_f64, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "lit_finish_str_bool", lkrt_lkmap_lit_finish_str_bool, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "lit_finish_str_dyn", lkrt_lkmap_lit_finish_str_dyn, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "lit_finish_i64_i64", lkrt_lkmap_lit_finish_i64_i64, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "lit_finish_i64_f64", lkrt_lkmap_lit_finish_i64_f64, WritesHost, [Ptr], Ptr, Constructs);
            // Iteration family (VM order by the layout mirror): pair lists
            // (`for pair in m`), keys/values snapshots (Mixed → dyn lists),
            // delete-with-removed-value (nil when absent).
            ("map_h", "str_i64_iter_pairs", lkrt_lkmap_str_i64_iter_pairs, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_i64_keys", lkrt_lkmap_str_i64_keys, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_i64_values", lkrt_lkmap_str_i64_values, WritesHost, [Ptr], Ptr, Constructs);
            // `clear` was the one container method the map lacked while the
            // list and the set both had it, so `m.clear()` dropped its module to
            // the VM. `Map<str, bool>` rides the `str_i64` carrier, so five
            // helpers cover the six map types the MIR distinguishes.
            ("map_h", "str_i64_clear", lkrt_lkmap_str_i64_clear, WritesHost, [Ptr], Nil, Borrowed);
            ("map_h", "i64_i64_clear", lkrt_lkmap_i64_i64_clear, WritesHost, [Ptr], Nil, Borrowed);
            ("map_h", "str_f64_clear", lkrt_lkmap_str_f64_clear, WritesHost, [Ptr], Nil, Borrowed);
            ("map_h", "i64_f64_clear", lkrt_lkmap_i64_f64_clear, WritesHost, [Ptr], Nil, Borrowed);
            ("map_h", "str_dyn_clear", lkrt_lkmap_str_dyn_clear, WritesHost, [Ptr], Nil, Borrowed);
            ("map_h", "str_i64_delete", lkrt_lkmap_str_i64_delete, WritesHost, [Ptr, StrPtr], DynVal, Borrowed);
            ("map_h", "str_f64_iter_pairs", lkrt_lkmap_str_f64_iter_pairs, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_f64_keys", lkrt_lkmap_str_f64_keys, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_f64_values", lkrt_lkmap_str_f64_values, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_f64_delete", lkrt_lkmap_str_f64_delete, WritesHost, [Ptr, StrPtr], DynVal, Borrowed);
            ("map_h", "str_bool_iter_pairs", lkrt_lkmap_str_bool_iter_pairs, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_bool_keys", lkrt_lkmap_str_bool_keys, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_bool_values", lkrt_lkmap_str_bool_values, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_bool_delete", lkrt_lkmap_str_bool_delete, WritesHost, [Ptr, StrPtr], DynVal, Borrowed);
            ("map_h", "str_dyn_iter_pairs", lkrt_lkmap_str_dyn_iter_pairs, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_dyn_keys", lkrt_lkmap_str_dyn_keys, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_dyn_values", lkrt_lkmap_str_dyn_values, WritesHost, [Ptr], Ptr, Constructs);
            ("map_h", "str_dyn_delete", lkrt_lkmap_str_dyn_delete, WritesHost, [Ptr, StrPtr], DynVal, Borrowed);
            ("list_h", "i64_to_dyn", lkrt_lklist_i64_to_dyn, WritesHost, [Ptr], Ptr, Constructs);
            // Typed map → `Map<str, Dyn>` conversion (cold: a typed map
            // crossing a `try$call` cell boundary boxes). Replayed inserts in
            // iteration order keep the layout — same keys, same order.
            ("list_h", "f64_to_dyn", lkrt_lklist_f64_to_dyn, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "str_to_dyn", lkrt_lklist_str_to_dyn, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "dyn_new", lkrt_lklist_dyn_new, WritesHost, [], Ptr, Constructs);
            ("list_h", "dyn_push", lkrt_lklist_dyn_push, WritesHost, [Ptr, DynVal], Nil, Borrowed);
            ("list_h", "dyn_at", lkrt_lklist_dyn_at, ReadsHost, [Ptr, I64], DynVal, Borrowed);
            ("list_h", "dyn_set", lkrt_lklist_dyn_set, WritesHost, [Ptr, I64, DynVal], Nil, Borrowed);
            ("list_h", "dyn_len", lkrt_lklist_dyn_len, ReadsHost, [Ptr], I64, Borrowed);
            ("list_h", "dyn_eq", lkrt_lklist_dyn_eq, ReadsHost, [Ptr, Ptr], I64, Borrowed);
            ("list_h", "dyn_chunk", lkrt_lklist_dyn_chunk, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "dyn_enumerate", lkrt_lklist_dyn_enumerate, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "dyn_zip", lkrt_lklist_dyn_zip, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "dyn_unique", lkrt_lklist_dyn_unique, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "dyn_flatten", lkrt_lklist_dyn_flatten, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "dyn_slice_from", lkrt_lklist_dyn_slice_from, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "dyn_contains", lkrt_lklist_dyn_contains, ReadsHost, [Ptr, DynVal], I64, Borrowed);
            ("list_h", "dyn_drop_last", lkrt_lklist_dyn_drop_last, WritesHost, [Ptr], Nil, Borrowed);
            ("list_h", "dyn_insert", lkrt_lklist_dyn_insert, WritesHost, [Ptr, I64, DynVal], Nil, Borrowed);
            ("list_h", "dyn_remove_at", lkrt_lklist_dyn_remove_at, WritesHost, [Ptr, I64], DynVal, Borrowed);
            ("list_h", "dyn_take", lkrt_lklist_dyn_take, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "dyn_skip", lkrt_lklist_dyn_skip, WritesHost, [Ptr, I64], Ptr, Constructs);
            ("list_h", "dyn_chain", lkrt_lklist_dyn_chain, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            // Boxed-element HOFs (`fn(LkDyn) -> LkDyn` / `-> bool` /
            // `fn(LkDyn, LkDyn) -> LkDyn` callbacks): the runtime-polymorphic
            // spellings of map/filter/reduce (typed receivers convert first).
            ("list_h", "dyn_map_fn", lkrt_lklist_dyn_map_fn, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "dyn_filter_fn", lkrt_lklist_dyn_filter_fn, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "dyn_reduce_fn", lkrt_lklist_dyn_reduce_fn, WritesHost, [Ptr, DynVal, Ptr], DynVal, Borrowed);
            ("list_h", "str_map_fn", lkrt_lklist_str_map_fn, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "str_filter_fn", lkrt_lklist_str_filter_fn, WritesHost, [Ptr, Ptr], Ptr, Constructs);
            ("list_h", "i64_unique", lkrt_lklist_i64_unique, WritesHost, [Ptr], Ptr, Constructs);
            ("list_h", "dyn_display", lkrt_lklist_dyn_display, WritesHost, [Ptr], StrPtr, Borrowed);
            // Native `Set` handles (VM `RuntimeSet`): boxed-key membership,
            // mutation, and size. Iteration/`values()` stays out (hash order).
            ("set", "new", lkrt_lkset_new, WritesHost, [], Ptr, Constructs);
            ("set", "from_str_list", lkrt_lkset_from_str_list, WritesHost, [Ptr], Ptr, Constructs);
            ("set", "from_i64_list", lkrt_lkset_from_i64_list, WritesHost, [Ptr], Ptr, Constructs);
            ("set", "from_dyn_list", lkrt_lkset_from_dyn_list, WritesHost, [Ptr], Ptr, Constructs);
            ("set", "has", lkrt_lkset_has, ReadsHost, [Ptr, DynVal], I64, Borrowed);
            ("set", "add", lkrt_lkset_add, WritesHost, [Ptr, DynVal], I64, Borrowed);
            ("set", "delete", lkrt_lkset_delete, WritesHost, [Ptr, DynVal], I64, Borrowed);
            ("set", "len", lkrt_lkset_len, ReadsHost, [Ptr], I64, Borrowed);
            ("set", "clear", lkrt_lkset_clear, WritesHost, [Ptr], Nil, Borrowed);
            ("set", "display", lkrt_lkset_display, WritesHost, [Ptr], StrPtr, Borrowed);
            ("set", "eq", lkrt_lkset_eq, ReadsHost, [Ptr, Ptr], I64, Borrowed);
            ("set", "iter", lkrt_lkset_iter, WritesHost, [Ptr], Ptr, Constructs);
            ("arith", "i64_div", lkrt_i64_div_checked, ReadsHost, [I64, I64], I64);
            ("arith", "i64_mod", lkrt_i64_mod_checked, ReadsHost, [I64, I64], I64);
            ("arith", "f64_div", lkrt_f64_div_checked, ReadsHost, [F64, F64], F64);
            ("arith", "f64_mod", lkrt_f64_mod_checked, ReadsHost, [F64, F64], F64);
            ("arith", "i64_shl", lkrt_i64_shl_checked, ReadsHost, [I64, I64], I64);
            ("arith", "i64_shr", lkrt_i64_shr_checked, ReadsHost, [I64, I64], I64);
            ("arith", "u64_shr", lkrt_u64_shr_checked, ReadsHost, [I64, I64], I64);
            ("arith", "u64_lt", lkrt_u64_lt, Pure, [I64, I64], I64);
            ("arith", "u64_div", lkrt_u64_div, ReadsHost, [I64, I64], I64);
            ("arith", "u64_rem", lkrt_u64_rem, ReadsHost, [I64, I64], I64);
            ("arith", "u64_to_f64", lkrt_u64_to_f64, Pure, [I64], F64);
        }
    };
}

/// Expands the ABI table into the [`ABI_FUNCTIONS`] const slice.
/// Resolves an entry's optional receiver contract, defaulting to the
/// conservative [`Receiver::Retained`].
macro_rules! receiver_or_default {
    () => {
        Receiver::Retained
    };
    ($role:ident) => {
        Receiver::$role
    };
}

macro_rules! define_abi_functions {
    ($( ($module:literal, $name:literal, $symbol:ident, $effect:ident, [$($param:ident),* $(,)?], $ret:ident $(, $role:ident)?) );* $(;)?) => {
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
                receiver: receiver_or_default!($($role)?),
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

    /// A call that answers something different each time may not be `Pure`.
    ///
    /// `Pure` is what the MIR CSE pass keys on: two `Pure` calls with equal
    /// arguments in one dominance scope become one. For a clock or a UUID that
    /// is a wrong answer, not a slow one — `let a = uuid.v4(); let b =
    /// uuid.v4();` would bind the same string twice. These entries take no
    /// arguments, which is exactly the case CSE collapses most eagerly, so the
    /// classification is pinned here rather than left to whoever adds the next
    /// one by copying a neighbouring row.
    #[test]
    fn nondeterministic_entries_are_not_pure() {
        for (module, name) in [
            ("uuid", "v4"),
            ("random", "int"),
            ("random", "float"),
            ("random", "bool"),
            ("random", "choice_i64"),
            ("random", "shuffle_i64"),
            ("os", "clock"),
            ("os", "epoch"),
            ("time", "now"),
            ("datetime", "now"),
        ] {
            let entry = find(module, name).expect("entry exists");
            assert!(
                !matches!(entry.effect, AbiEffect::Pure),
                "{module}.{name} is Pure, so CSE may merge two calls that must answer differently"
            );
        }
    }

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

    /// The receiver contract drives an early `free`, so these specific
    /// classifications are load-bearing. Each was checked against the `lkrt`
    /// implementation; this test keeps an edit from silently reclassifying one.
    #[test]
    fn receiver_contracts_match_the_audited_implementations() {
        let receiver = |m, n| find(m, n).expect("known entry").receiver;

        // Fresh arena handles: releasing their result is the whole point.
        assert_eq!(receiver("list_h", "i64_new"), Receiver::Constructs);
        assert_eq!(receiver("map_h", "lit_new"), Receiver::Constructs);
        // `sort`/`reverse` clone into a new handle rather than mutating.
        assert_eq!(receiver("list_h", "i64_sort"), Receiver::Constructs);
        assert_eq!(receiver("list_h", "i64_reverse"), Receiver::Constructs);
        // Reached through `pair_list`/`arena_handle` helpers, not a literal
        // `arena_handle` call in the entry's own body.
        assert_eq!(receiver("map_h", "str_i64_iter_pairs"), Receiver::Constructs);
        assert_eq!(receiver("map_h", "str_i64_keys"), Receiver::Constructs);

        // Read/mutate in place: safe to release the receiver afterwards.
        assert_eq!(receiver("list_h", "i64_len"), Receiver::Borrowed);
        assert_eq!(receiver("list_h", "i64_push"), Receiver::Borrowed);
        assert_eq!(receiver("map_h", "str_dyn_set"), Receiver::Borrowed);

        // The two audited exceptions, both of which a name-based rule gets
        // wrong. `obj_mark` records the handle's address in a global table;
        // `dyn.as_list` returns an *existing* handle, so treating it as a
        // constructor would free a container someone else still owns.
        assert_eq!(receiver("map_h", "obj_mark"), Receiver::Retained);
        assert_eq!(receiver("dyn", "as_list"), Receiver::Retained);

        // Anything unannotated stays conservative.
        assert_eq!(receiver("dyn", "from_list"), Receiver::Retained);
        assert_eq!(receiver("json", "parse"), Receiver::Retained);
    }

    /// Every entry that returns a raw pointer either declares itself a
    /// constructor or stays `Retained`; a `Borrowed` pointer-returning entry
    /// would be a classification mistake (it hands back a handle nobody owns).
    ///
    /// The converse direction matters just as much: `Constructs` is what the
    /// MIR scope-drop pass reads to decide a value is a fresh, releasable
    /// handle, so an entry claiming it without returning a pointer would hand
    /// the pass a non-handle to free.
    #[test]
    fn pointer_returning_entries_are_not_marked_borrowed() {
        for f in ABI_FUNCTIONS {
            if f.result == AbiType::Ptr {
                assert_ne!(
                    f.receiver,
                    Receiver::Borrowed,
                    "{}.{} returns a handle but is marked Borrowed",
                    f.module,
                    f.name
                );
            }
            if f.receiver == Receiver::Constructs {
                assert_eq!(
                    f.result,
                    AbiType::Ptr,
                    "{}.{} is marked Constructs but does not return a handle",
                    f.module,
                    f.name
                );
            }
        }
    }
}
