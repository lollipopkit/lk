use super::*;

/// Module-object metadata: how one stdlib module name binds. Single source
/// of truth — the bare-`GetGlobal` whitelist and the submodule import
/// routing both derive from this table (adding a module is one row here
/// plus its [`MODULE_ABI`] members, never a scattered edit).
pub(crate) struct ModuleRow {
    pub(crate) name: &'static str,
    /// A bare `GetGlobal name` resolves to the module object. Two-level
    /// exports (`chan::close`) route by the qualified name instead, and
    /// submodules bind through their parent's import — both stay `false`.
    pub(crate) bare_global: bool,
    /// `use { name } from <parent>` binds a *submodule object* (member
    /// reads route through `GlobalRef::Module`), not a function.
    pub(crate) submodule_of: Option<&'static str>,
}

pub(crate) const MODULE_TABLE: &[ModuleRow] = &[
    ModuleRow {
        name: "os",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "time",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "env",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "math",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "fs",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "process",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "datetime",
        bare_global: true,
        submodule_of: None,
    },
    // `std` is a **submodule of `io`** (`use { std } from io;`), not a bare
    // global: a bare `std` does not resolve at all. This row claimed otherwise —
    // harmlessly, because it has no members here, but this table is documented
    // as the single source of truth for how a module name binds.
    ModuleRow {
        name: "std",
        bare_global: false,
        submodule_of: Some("io"),
    },
    ModuleRow {
        name: "iter",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "string",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "path",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "task",
        bare_global: true,
        submodule_of: None,
    },
    // `chan`'s other members arrive pre-flattened (`GetGlobal "chan::close"`),
    // because they are registered under those names; `chan.new` is an ordinary
    // module export, so it arrives as the module object plus a `GetIndex`. The
    // module needed a row here for that shape to resolve at all — without it
    // `use chan; chan.new(1)` dropped its module to the VM while the global
    // `chan(1)` lowered.
    ModuleRow {
        name: "chan",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "stream",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "bytes",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "hash",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "regex",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "uuid",
        bare_global: true,
        submodule_of: None,
    },
    // The submodule *parents*. They have no typed members of their own, but the
    // name has to bind for `encoding.json.parse(s)` to reach the submodule at
    // all — without these rows the chain stopped at the first dot and the whole
    // program fell back, while `use { json } from encoding;` lowered.
    ModuleRow {
        name: "encoding",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "net",
        bare_global: true,
        submodule_of: None,
    },
    ModuleRow {
        name: "io",
        bare_global: true,
        submodule_of: None,
    },
    // `encoding`/`net`/`io` submodules.
    ModuleRow {
        name: "json",
        bare_global: false,
        submodule_of: Some("encoding"),
    },
    ModuleRow {
        name: "yaml",
        bare_global: false,
        submodule_of: Some("encoding"),
    },
    ModuleRow {
        name: "toml",
        bare_global: false,
        submodule_of: Some("encoding"),
    },
    ModuleRow {
        name: "base64",
        bare_global: false,
        submodule_of: Some("encoding"),
    },
    ModuleRow {
        name: "hex",
        bare_global: false,
        submodule_of: Some("encoding"),
    },
    ModuleRow {
        name: "url",
        bare_global: false,
        submodule_of: Some("encoding"),
    },
    ModuleRow {
        name: "socket",
        bare_global: false,
        submodule_of: Some("net"),
    },
    ModuleRow {
        name: "tcp",
        bare_global: false,
        submodule_of: Some("net"),
    },
    ModuleRow {
        name: "udp",
        bare_global: false,
        submodule_of: Some("net"),
    },
    ModuleRow {
        name: "file",
        bare_global: false,
        submodule_of: Some("io"),
    },
];

/// A bare `GetGlobal` of this name is a stdlib module object.
pub(crate) fn module_global(name: &str) -> bool {
    MODULE_TABLE.iter().any(|row| row.name == name && row.bare_global)
}

/// `use { member } from parent` binds a submodule object (not a function).
pub(crate) fn is_submodule(parent: &str, member: &str) -> bool {
    MODULE_TABLE
        .iter()
        .any(|row| row.name == member && row.submodule_of == Some(parent))
}

/// One natively lowerable `module.member` call: the lkrt `AbiRef`, the exact
/// positional argument types, and the return type. Members not in the table
/// are not natively lowerable (yet) and the program falls back.
/// (`math.floor` dispatches on its argument type in [`lower_module_call`].)
///
/// Every row must be VM-exact: same value semantics *and* the same display
/// (the differential corpora compare stdout byte-for-byte).
pub(crate) struct ModuleAbiRow {
    pub(crate) module: &'static str,
    pub(crate) member: &'static str,
    pub(crate) abi: AbiRef,
    pub(crate) args: &'static [Ty],
    pub(crate) ret: Ty,
    /// The names of the trailing parameters a caller may pass by name, in
    /// frame order — the stdlib export's `named(...)` list.
    ///
    /// Empty means positional-only, which is most members. A member that
    /// declares names can be *called* by name, and that call is a different
    /// opcode (`CallNamed`) carrying its arguments in caller order; without
    /// these the permutation is unknown and the whole program falls back. The
    /// stdlib marks a member `named` precisely when the positional spelling is
    /// hard to read — `regex.replace(pattern, text, replacement)` is three
    /// strings with the subject in the middle — so the spelling that lowers
    /// would have been the one nobody is meant to write.
    pub(crate) named: &'static [&'static str],
}

pub(crate) const fn abi_row(
    module: &'static str,
    member: &'static str,
    abi: AbiRef,
    args: &'static [Ty],
    ret: Ty,
) -> ModuleAbiRow {
    ModuleAbiRow {
        module,
        member,
        abi,
        args,
        ret,
        named: &[],
    }
}

/// [`abi_row`] for a member whose trailing parameters may be passed by name.
pub(crate) const fn abi_row_named(
    module: &'static str,
    member: &'static str,
    abi: AbiRef,
    args: &'static [Ty],
    ret: Ty,
    named: &'static [&'static str],
) -> ModuleAbiRow {
    ModuleAbiRow {
        module,
        member,
        abi,
        args,
        ret,
        named,
    }
}

pub(crate) const MODULE_ABI: &[ModuleAbiRow] = &[
    // Monotonic in-process seconds (f64) — both sides anchor to first use.
    abi_row("os", "clock", AbiRef::new("os", "clock"), &[], Ty::F64),
    // Unix epoch milliseconds.
    abi_row("os", "epoch", AbiRef::new("os", "epoch"), &[], Ty::I64),
    // Monotonic milliseconds / sleep-for-milliseconds.
    abi_row("time", "now", AbiRef::new("time", "now"), &[], Ty::I64),
    abi_row("time", "sleep", AbiRef::new("time", "sleep"), &[Ty::I64], Ty::Nil),
    // Environment lookup with a default; both sides return an owned string.
    abi_row(
        "env",
        "get_or",
        AbiRef::new("env", "get_or"),
        &[Ty::Str, Ty::Str],
        Ty::Str,
    ),
    // System info strings (allocated per call, arena-owned).
    abi_row("os", "hostname", AbiRef::new("os", "hostname"), &[], Ty::Str),
    abi_row("os", "arch", AbiRef::new("os", "arch"), &[], Ty::Str),
    abi_row("os", "os", AbiRef::new("os", "name"), &[], Ty::Str),
    abi_row("process", "cwd", AbiRef::new("process", "cwd"), &[], Ty::Str),
    abi_row("fs", "temp_dir", AbiRef::new("fs", "temp_dir"), &[], Ty::Str),
    // The rest of `fs`. Every one of these had an lkrt implementation and an ABI
    // row already — and no row here, so nothing could reach them: the runtime
    // was built, and three of its error messages had drifted from the VM's
    // without anything noticing, because an unreachable path is an *unverified*
    // path, not a spare one.
    abi_row(
        "fs",
        "read_to_string",
        AbiRef::new("fs", "read_to_string"),
        &[Ty::Str],
        Ty::Str,
    ),
    // `fs.write(path, data)` takes `Bytes | String` — one row per carrier.
    abi_row(
        "fs",
        "write",
        AbiRef::new("fs", "write_str"),
        &[Ty::Str, Ty::Str],
        Ty::Bool,
    ),
    abi_row(
        "fs",
        "write",
        AbiRef::new("fs", "write_bytes"),
        &[Ty::Str, Ty::Bytes],
        Ty::Bool,
    ),
    abi_row(
        "fs",
        "read_dir",
        AbiRef::new("fs", "read_dir_list"),
        &[Ty::Str],
        Ty::ListStr,
    ),
    // `String?`, so it arrives boxed: a resolved path that is not UTF-8 is nil.
    abi_row(
        "fs",
        "canonicalize",
        AbiRef::new("fs", "canonicalize"),
        &[Ty::Str],
        Ty::Dyn,
    ),
    // `fs.metadata` and `env.vars` answer string-keyed maps of mixed values —
    // `Ty::MapStrDyn`, built on the lkrt side through the same two-stage
    // construction the VM uses, because a map's iteration order is what
    // `println` prints.
    abi_row(
        "fs",
        "metadata",
        AbiRef::new("fs", "metadata_map"),
        &[Ty::Str],
        Ty::MapStrDyn,
    ),
    abi_row("env", "vars", AbiRef::new("env", "vars_map"), &[], Ty::MapStrDyn),
    abi_row("fs", "is_file", AbiRef::new("fs", "is_file"), &[Ty::Str], Ty::Bool),
    abi_row("fs", "is_dir", AbiRef::new("fs", "is_dir"), &[Ty::Str], Ty::Bool),
    abi_row(
        "fs",
        "append",
        AbiRef::new("fs", "append_str"),
        &[Ty::Str, Ty::Str],
        Ty::Bool,
    ),
    abi_row(
        "fs",
        "append",
        AbiRef::new("fs", "append_bytes"),
        &[Ty::Str, Ty::Bytes],
        Ty::Bool,
    ),
    abi_row(
        "fs",
        "create_dir",
        AbiRef::new("fs", "create_dir"),
        &[Ty::Str],
        Ty::Bool,
    ),
    abi_row(
        "fs",
        "create_dir_all",
        AbiRef::new("fs", "create_dir_all"),
        &[Ty::Str],
        Ty::Bool,
    ),
    // The `remove_*` trio answers `false` for a path that was not there, and
    // raises for anything else.
    abi_row(
        "fs",
        "remove_file",
        AbiRef::new("fs", "remove_file"),
        &[Ty::Str],
        Ty::Bool,
    ),
    abi_row(
        "fs",
        "remove_dir",
        AbiRef::new("fs", "remove_dir"),
        &[Ty::Str],
        Ty::Bool,
    ),
    abi_row(
        "fs",
        "remove_dir_all",
        AbiRef::new("fs", "remove_dir_all"),
        &[Ty::Str],
        Ty::Bool,
    ),
    abi_row(
        "fs",
        "rename",
        AbiRef::new("fs", "rename"),
        &[Ty::Str, Ty::Str],
        Ty::Bool,
    ),
    // `copy` answers the byte count, not a bool.
    abi_row("fs", "copy", AbiRef::new("fs", "copy"), &[Ty::Str, Ty::Str], Ty::I64),
    abi_row("env", "has", AbiRef::new("env", "has"), &[Ty::Str], Ty::Bool),
    // Sorted entry names as List<str> (the VM's exact shape).
    abi_row(
        "fs",
        "read_dir",
        AbiRef::new("fs", "read_dir_list"),
        &[Ty::Str],
        Ty::ListStr,
    ),
    abi_row("fs", "exists", AbiRef::new("fs", "exists"), &[Ty::Str], Ty::Bool),
    // Answers a `Bytes` value. It had an ABI entry and no row, because there was
    // no type to give it.
    abi_row("fs", "read", AbiRef::new("fs", "read"), &[Ty::Str], Ty::Bytes),
    // chrono-backed datetime (byte-identical to the stdlib module).
    abi_row("datetime", "now", AbiRef::new("datetime", "now"), &[], Ty::I64),
    abi_row(
        "datetime",
        "format",
        AbiRef::new("datetime", "format"),
        &[Ty::I64, Ty::Str],
        Ty::Str,
    ),
    abi_row(
        "datetime",
        "parse",
        AbiRef::new("datetime", "parse"),
        &[Ty::Str, Ty::Str],
        Ty::I64,
    ),
    abi_row(
        "datetime",
        "day_of_week",
        AbiRef::new("datetime", "day_of_week"),
        &[Ty::I64],
        Ty::I64,
    ),
    abi_row(
        "datetime",
        "day_of_year",
        AbiRef::new("datetime", "day_of_year"),
        &[Ty::I64],
        Ty::I64,
    ),
    // Float-typed math (Number args f64-promote at the call site). `sqrt`
    // aborts on a negative argument (the stdlib module's loud error).
    abi_row("math", "sqrt", AbiRef::new("math", "sqrt"), &[Ty::F64], Ty::F64),
    abi_row("math", "sin", AbiRef::new("math", "sin"), &[Ty::F64], Ty::F64),
    abi_row("math", "cos", AbiRef::new("math", "cos"), &[Ty::F64], Ty::F64),
    abi_row("math", "tan", AbiRef::new("math", "tan"), &[Ty::F64], Ty::F64),
    // `asin`/`acos` reject outside `-1..=1`, the log family rejects
    // non-positive: the guards live in the helpers so both back ends raise the
    // stdlib module's own sentence.
    abi_row("math", "asin", AbiRef::new("math", "asin"), &[Ty::F64], Ty::F64),
    abi_row("math", "acos", AbiRef::new("math", "acos"), &[Ty::F64], Ty::F64),
    abi_row("math", "atan", AbiRef::new("math", "atan"), &[Ty::F64], Ty::F64),
    abi_row(
        "math",
        "atan2",
        AbiRef::new("math", "atan2"),
        &[Ty::F64, Ty::F64],
        Ty::F64,
    ),
    abi_row("math", "log", AbiRef::new("math", "log"), &[Ty::F64], Ty::F64),
    abi_row("math", "log10", AbiRef::new("math", "log10"), &[Ty::F64], Ty::F64),
    abi_row("math", "log2", AbiRef::new("math", "log2"), &[Ty::F64], Ty::F64),
    // `clamp` is `Int`-only in the module schema, so no f64 promotion here.
    abi_row_named(
        "math",
        "clamp",
        AbiRef::new("math", "clamp_i64"),
        &[Ty::I64, Ty::I64, Ty::I64],
        Ty::I64,
        &["min", "max"],
    ),
    abi_row("math", "exp", AbiRef::new("math", "exp"), &[Ty::F64], Ty::F64),
    abi_row("math", "pow", AbiRef::new("math", "pow"), &[Ty::F64, Ty::F64], Ty::F64),
    abi_row(
        "math",
        "hypot",
        AbiRef::new("math", "hypot"),
        &[Ty::F64, Ty::F64],
        Ty::F64,
    ),
    abi_row("math", "cbrt", AbiRef::new("math", "cbrt"), &[Ty::F64], Ty::F64),
    // Only a Float NaN is true; an Int argument f64-promotes (never NaN),
    // exactly the module's `matches!(.., Float(v) if v.is_nan())`.
    abi_row("math", "is_nan", AbiRef::new("math", "is_nan"), &[Ty::F64], Ty::Bool),
    // The `path` module's fixed-arity members. `parent`/`file_name`/`file_stem`/
    // `extension` answer `String?`, which arrives boxed — the convention
    // `string.strip_prefix` established.
    abi_row("path", "parent", AbiRef::new("path", "parent"), &[Ty::Str], Ty::Dyn),
    abi_row(
        "path",
        "file_name",
        AbiRef::new("path", "file_name"),
        &[Ty::Str],
        Ty::Dyn,
    ),
    abi_row(
        "path",
        "file_stem",
        AbiRef::new("path", "file_stem"),
        &[Ty::Str],
        Ty::Dyn,
    ),
    abi_row(
        "path",
        "extension",
        AbiRef::new("path", "extension"),
        &[Ty::Str],
        Ty::Dyn,
    ),
    abi_row(
        "path",
        "with_extension",
        AbiRef::new("path", "with_extension"),
        &[Ty::Str, Ty::Str],
        Ty::Str,
    ),
    abi_row(
        "path",
        "is_absolute",
        AbiRef::new("path", "is_absolute"),
        &[Ty::Str],
        Ty::Bool,
    ),
    abi_row(
        "path",
        "components",
        AbiRef::new("path", "components"),
        &[Ty::Str],
        Ty::ListStr,
    ),
    abi_row("path", "sep", AbiRef::new("path", "sep"), &[], Ty::Str),
    abi_row("path", "delimiter", AbiRef::new("path", "delimiter"), &[], Ty::Str),
    // String-or-nil results arrive boxed (`String?` in the module schema).
    abi_row(
        "string",
        "strip_prefix",
        AbiRef::new("str", "strip_prefix"),
        &[Ty::Str, Ty::Str],
        Ty::Dyn,
    ),
    abi_row(
        "string",
        "strip_suffix",
        AbiRef::new("str", "strip_suffix"),
        &[Ty::Str, Ty::Str],
        Ty::Dyn,
    ),
    abi_row(
        "string",
        "count",
        AbiRef::new("str", "count"),
        &[Ty::Str, Ty::Str],
        Ty::I64,
    ),
    // The module spelling counts bytes (`str::len`), unlike `.len()`.
    abi_row("string", "len", AbiRef::new("str", "byte_len"), &[Ty::Str], Ty::I64),
    // The module spelling of `s.slice(a, b)`, which the method path has always
    // lowered. Two spellings of one operation, and only one of them was fast.
    // `string.replace(text, pattern, with)` — the three-argument form. The
    // fourth parameter `all` defaults to true, which is what `str::replace`
    // does, so a call that omits it lowers; a call that passes `all` has a
    // different arity and no row, and falls back.
    abi_row_named(
        "string",
        "replace",
        AbiRef::new("str", "replace"),
        &[Ty::Str, Ty::Str, Ty::Str],
        Ty::Str,
        // The stdlib declares three names; this row is the arity that leaves
        // `all` at its default. A call that does pass `all` has four arguments,
        // finds no row, and falls back — which is why the *names* list stays
        // whole while the `args` list does not.
        &["pattern", "with", "all"],
    ),
    abi_row_named(
        "string",
        "slice",
        AbiRef::new("str", "slice_chars"),
        &[Ty::Str, Ty::I64, Ty::I64],
        Ty::Str,
        &["start", "end"],
    ),
    abi_row(
        "string",
        "capitalize",
        AbiRef::new("str", "capitalize"),
        &[Ty::Str],
        Ty::Str,
    ),
    abi_row("string", "title", AbiRef::new("str", "title"), &[Ty::Str], Ty::Str),
    // Text → number. `to_int` is not here: its base is optional, so it is
    // materialized in `lower_module` instead of split across two rows.
    abi_row(
        "string",
        "to_float",
        AbiRef::new("str", "to_float"),
        &[Ty::Str],
        Ty::Dyn,
    ),
    // Native channels/goroutines (plan H): channel/task values are i64
    // ids; blocking semantics + raises live in lkrt.
    abi_row("chan", "close", AbiRef::new("chan", "close"), &[Ty::I64], Ty::Nil),
    abi_row("chan", "len", AbiRef::new("chan", "len"), &[Ty::I64], Ty::I64),
    abi_row("chan", "capacity", AbiRef::new("chan", "capacity"), &[Ty::I64], Ty::I64),
    abi_row(
        "chan",
        "is_closed",
        AbiRef::new("chan", "is_closed"),
        &[Ty::I64],
        Ty::Bool,
    ),
    abi_row(
        "chan",
        "try_send",
        AbiRef::new("chan", "try_send"),
        &[Ty::I64, Ty::Dyn],
        Ty::Bool,
    ),
    abi_row("chan", "try_recv", AbiRef::new("chan", "try_recv"), &[Ty::I64], Ty::Dyn),
    // The blocking pair. Both were reachable only as bare globals until the
    // module grew them, so neither had a row here either.
    abi_row(
        "chan",
        "send",
        AbiRef::new("chan", "send"),
        &[Ty::I64, Ty::Dyn],
        Ty::Nil,
    ),
    abi_row("chan", "recv", AbiRef::new("chan", "recv"), &[Ty::I64], Ty::Dyn),
    // The module spelling of the global `chan(capacity)`. Same lkrt entry; the
    // optional type-string argument is a checker hint the VM drops too, so only
    // the one-argument form has a row (two args takes the generic path).
    abi_row("chan", "new", AbiRef::new("chan", "new"), &[Ty::I64], Ty::I64),
    abi_row("task", "await", AbiRef::new("rt", "task_await"), &[Ty::I64], Ty::Dyn),
    // `encoding` submodules (VM `de.rs` mirrored in lkrt).
    abi_row("json", "parse", AbiRef::new("json", "parse"), &[Ty::Str], Ty::Dyn),
    // `base64`/`hex`/`url`. `encode` takes `Bytes | String` in the language, so
    // it is two rows — the second used to be missing, and
    // `base64.encode(bytes.from_string("hi"))` therefore ran on the bridge while
    // the same call on a string ran native.
    abi_row("base64", "encode", AbiRef::new("base64", "encode"), &[Ty::Str], Ty::Str),
    abi_row(
        "base64",
        "encode",
        AbiRef::new("base64", "encode_bytes"),
        &[Ty::Bytes],
        Ty::Str,
    ),
    abi_row("hex", "encode", AbiRef::new("hex", "encode"), &[Ty::Str], Ty::Str),
    abi_row(
        "hex",
        "encode",
        AbiRef::new("hex", "encode_bytes"),
        &[Ty::Bytes],
        Ty::Str,
    ),
    // `hash`, both carriers of every member. The digests come from the same
    // crates the stdlib module uses (`sha2`/`sha1`/`crc32fast`); `fnv64` is the
    // one loop that exists twice, and `lkrt`'s `vm_mirror` conformance test is
    // what keeps the two spellings equal.
    // `regex`. `find` answers `Map?` and `captures` answers `List?`, so both
    // arrive boxed; `find_all` is a dyn list of match maps. Each map is built
    // through the VM's own two-stage construction (`str_dyn_map_mirrored`) —
    // its keys are `text`, `start`, `end`, and that insertion order is what
    // `println` prints.
    abi_row(
        "regex",
        "find",
        AbiRef::new("regex", "find"),
        &[Ty::Str, Ty::Str],
        Ty::Dyn,
    ),
    abi_row(
        "regex",
        "find_all",
        AbiRef::new("regex", "find_all"),
        &[Ty::Str, Ty::Str],
        Ty::ListDyn,
    ),
    abi_row(
        "regex",
        "captures",
        AbiRef::new("regex", "captures"),
        &[Ty::Str, Ty::Str],
        Ty::Dyn,
    ),
    abi_row(
        "regex",
        "is_match",
        AbiRef::new("regex", "is_match"),
        &[Ty::Str, Ty::Str],
        Ty::Bool,
    ),
    abi_row(
        "regex",
        "split",
        AbiRef::new("regex", "split"),
        &[Ty::Str, Ty::Str],
        Ty::ListStr,
    ),
    abi_row_named(
        "regex",
        "replace",
        AbiRef::new("regex", "replace"),
        &[Ty::Str, Ty::Str, Ty::Str],
        Ty::Str,
        &["text", "replacement"],
    ),
    // `uuid`. `v4` has no arguments and a different answer every call — see the
    // ABI schema for why it must not be `Pure`.
    abi_row("uuid", "v4", AbiRef::new("uuid", "v4"), &[], Ty::Str),
    abi_row("uuid", "parse", AbiRef::new("uuid", "parse"), &[Ty::Str], Ty::Str),
    abi_row(
        "uuid",
        "is_valid",
        AbiRef::new("uuid", "is_valid"),
        &[Ty::Str],
        Ty::Bool,
    ),
    abi_row("hash", "sha256", AbiRef::new("hash", "sha256_str"), &[Ty::Str], Ty::Str),
    abi_row(
        "hash",
        "sha256",
        AbiRef::new("hash", "sha256_bytes"),
        &[Ty::Bytes],
        Ty::Str,
    ),
    abi_row("hash", "sha1", AbiRef::new("hash", "sha1_str"), &[Ty::Str], Ty::Str),
    abi_row("hash", "sha1", AbiRef::new("hash", "sha1_bytes"), &[Ty::Bytes], Ty::Str),
    abi_row("hash", "crc32", AbiRef::new("hash", "crc32_str"), &[Ty::Str], Ty::I64),
    abi_row(
        "hash",
        "crc32",
        AbiRef::new("hash", "crc32_bytes"),
        &[Ty::Bytes],
        Ty::I64,
    ),
    abi_row("hash", "fnv64", AbiRef::new("hash", "fnv64_str"), &[Ty::Str], Ty::I64),
    abi_row(
        "hash",
        "fnv64",
        AbiRef::new("hash", "fnv64_bytes"),
        &[Ty::Bytes],
        Ty::I64,
    ),
    abi_row(
        "url",
        "encode_component",
        AbiRef::new("url", "encode_component"),
        &[Ty::Str],
        Ty::Str,
    ),
    abi_row(
        "url",
        "decode_component",
        AbiRef::new("url", "decode_component"),
        &[Ty::Str],
        Ty::Str,
    ),
    abi_row("yaml", "parse", AbiRef::new("yaml", "parse"), &[Ty::Str], Ty::Dyn),
    abi_row("toml", "parse", AbiRef::new("toml", "parse"), &[Ty::Str], Ty::Dyn),
    // `net` submodules + `bytes` (the lkrt tcp family predates this).
    abi_row(
        "socket",
        "addr",
        AbiRef::new("socket", "addr"),
        &[Ty::Str, Ty::I64],
        Ty::Str,
    ),
    abi_row("tcp", "connect", AbiRef::new("tcp", "connect"), &[Ty::Str], Ty::I64),
    abi_row(
        "tcp",
        "write",
        AbiRef::new("tcp", "write_str"),
        &[Ty::I64, Ty::Str],
        Ty::I64,
    ),
    // Answers a `Bytes` **value**, not the one-shot host handle it used to: a
    // `Bytes` you can only read once is not the language's `Bytes`.
    abi_row(
        "tcp",
        "read",
        AbiRef::new("tcp", "read"),
        &[Ty::I64, Ty::I64],
        Ty::Bytes,
    ),
    abi_row("tcp", "close", AbiRef::new("tcp", "close"), &[Ty::I64], Ty::I64),
    // The `bytes` module over the `Bytes` handle. `from_list` / `to_list` /
    // `slice` need list interop and stay out for now — a member with no row is
    // an ordinary fallback.
    abi_row(
        "bytes",
        "from_string",
        AbiRef::new("bytes_h", "from_str"),
        &[Ty::Str],
        Ty::Bytes,
    ),
    abi_row("bytes", "len", AbiRef::new("bytes_h", "len"), &[Ty::Bytes], Ty::I64),
    abi_row(
        "bytes",
        "from_list",
        AbiRef::new("bytes_h", "from_i64_list"),
        &[Ty::ListI64],
        Ty::Bytes,
    ),
    abi_row(
        "bytes",
        "to_list",
        AbiRef::new("bytes_h", "to_i64_list"),
        &[Ty::Bytes],
        Ty::ListI64,
    ),
    abi_row(
        "bytes",
        "is_empty",
        AbiRef::new("bytes_h", "is_empty"),
        &[Ty::Bytes],
        Ty::Bool,
    ),
    abi_row(
        "bytes",
        "get",
        AbiRef::new("bytes_h", "get"),
        &[Ty::Bytes, Ty::I64],
        Ty::Dyn,
    ),
    abi_row_named(
        "bytes",
        "slice",
        AbiRef::new("bytes_h", "slice"),
        &[Ty::Bytes, Ty::I64, Ty::I64],
        Ty::Bytes,
        &["start", "end"],
    ),
    abi_row(
        "bytes",
        "concat",
        AbiRef::new("bytes_h", "concat"),
        &[Ty::Bytes, Ty::Bytes],
        Ty::Bytes,
    ),
    abi_row(
        "bytes",
        "to_string_lossy",
        AbiRef::new("bytes_h", "utf8_lossy"),
        &[Ty::Bytes],
        Ty::Str,
    ),
    abi_row(
        "base64",
        "decode",
        AbiRef::new("base64", "decode"),
        &[Ty::Str],
        Ty::Bytes,
    ),
    abi_row("hex", "decode", AbiRef::new("hex", "decode"), &[Ty::Str], Ty::Bytes),
    abi_row(
        "bytes",
        "to_string_utf8",
        // Was `AbiRef::new("bytes", "to_string_utf8")` over an `i64` *host*
        // handle — the one-shot kind the `tcp` path reads with `take_bytes`.
        // A `Bytes` in the language is a value you may read twice, so it is an
        // arena handle now and this row points at that.
        AbiRef::new("bytes_h", "utf8"),
        &[Ty::Bytes],
        Ty::Str,
    ),
];

/// Every row for one member, in table order.
///
/// A stdlib member may accept more than one carrier — `hash.sha256(data)` and
/// `base64.encode(data)` each take `Bytes | String`, and a `Bytes` is a
/// different native argument than a `Str`, so it is a different row. The
/// caller ([`lower_module_call`]) picks by the argument types it actually has.
///
/// Before this existed the table was keyed by name alone, so a two-carrier
/// member got whichever row was written first and the other carrier fell back
/// silently: `base64.encode(bytes.from_string("hi"))` ran on the bridge while
/// the same call on a string ran native. That is the same "one operation, N
/// carriers, only some of them finished" shape the list methods had.
pub(crate) fn module_call_abi_rows<'a>(
    module: &'a str,
    name: &'a str,
) -> impl Iterator<Item = &'static ModuleAbiRow> + 'a {
    MODULE_ABI
        .iter()
        .filter(move |row| row.module == module && row.member == name)
}

/// Every member whose row carries a `named(...)` list, for the CLI test that
/// compares this table against the stdlib's own declaration.
///
/// The list is a copy — `aot/lower` cannot read the stdlib signature registry,
/// which is populated at run time by whoever links the standard library, and a
/// lowering that silently degrades when that has not happened yet is worse than
/// a copy with a test on it.
pub fn named_parameter_rows() -> impl Iterator<Item = (&'static str, &'static str, &'static [&'static str])> {
    MODULE_ABI
        .iter()
        .filter(|row| !row.named.is_empty())
        .map(|row| (row.module, row.member, row.named))
}

/// Whether a row's declared parameter type accepts an argument the lowering
/// actually holds — the same three rules [`lower_module_call`] then applies
/// when it materialises the argument: exact, `Dyn` takes anything (it boxes),
/// and `F64` takes an `I64` (the stdlib's `number_arg` promotion).
pub(crate) fn abi_param_accepts(want: Ty, got: Ty) -> bool {
    want == got || want == Ty::Dyn || (want == Ty::F64 && got == Ty::I64)
}

/// Method-name roles across the lowering — the single source of truth the
/// `Dyn`-receiver unbox guards, the string-list lookahead, and the
/// `iter`/`stream` module-spelling forwarders all derive from. Adding a
/// stdlib method with any of these behaviours is one row here.
pub(crate) struct MethodRow {
    pub(crate) name: &'static str,
    /// A `Dyn` receiver unboxes through `dyn.as_list` (list-only name; a
    /// non-list tag aborts, the VM's method-on-wrong-type loud error).
    pub(crate) unbox_list: bool,
    /// A `Dyn` receiver unboxes through `dyn.as_map` (map-only name).
    /// Names shared with other receivers (`get`) stay boxed and reject.
    pub(crate) unbox_map: bool,
    /// A string-list receiver's result is still a string list (the
    /// `strlist_regs` lookahead keeps tracking through the call).
    pub(crate) strlist: bool,
    /// `iter.name(xs, …)` / `stream.name(…)` forwards to the method
    /// lowering (the VM routes both spellings through core_methods).
    pub(crate) forward: bool,
}

pub(crate) const fn method_row(
    name: &'static str,
    unbox_list: bool,
    unbox_map: bool,
    strlist: bool,
    forward: bool,
) -> MethodRow {
    MethodRow {
        name,
        unbox_list,
        unbox_map,
        strlist,
        forward,
    }
}

#[rustfmt::skip]
pub(crate) const METHOD_TABLE: &[MethodRow] = &[
    //          name         unbox_list unbox_map strlist forward
    method_row("map",        true,      false,    true,   true),
    method_row("filter",     true,      false,    true,   true),
    method_row("reduce",     true,      false,    false,  true),
    method_row("take",       true,      false,    true,   true),
    method_row("skip",       true,      false,    true,   true),
    method_row("concat",     true,      false,    true,   false),
    method_row("unique",     true,      false,    true,   true),
    method_row("sort",       true,      false,    true,   false),
    method_row("reverse",    true,      false,    true,   false),
    method_row("slice",      false,     false,    true,   false),
    method_row("enumerate",  false,     false,    false,  true),
    method_row("zip",        false,     false,    false,  true),
    method_row("chain",      false,     false,    false,  true),
    method_row("flatten",    false,     false,    false,  true),
    method_row("chunk",      false,     false,    false,  true),
    method_row("has",        false,     true,     false,  false),
    method_row("keys",       false,     true,     false,  false),
    method_row("values",     false,     true,     false,  false),
    method_row("delete",     false,     true,     false,  false),
    method_row("remove",     false,     true,     false,  false),
];

/// Whether `module.name(receiver, …)` is the method `receiver.name(…)`.
///
/// The VM routes both spellings through the same `core_methods`, so the
/// lowering has one job: put the receiver where the method arm expects it. This
/// used to be spelled `matches!(module, "iter" | "stream")` at the call site —
/// a list of two, not a rule — so every `string` module function fell back
/// while its method spelling lowered. `string.trim(s)` and `s.trim()` are the
/// same call; which one a program wrote decided whether it stayed native.
///
/// `string`'s names are listed rather than "anything the method table knows",
/// because deciding by *trying* `lower_method_dispatch` would emit instructions
/// before finding out — the mistake `lower_conditional` made and paid for.
pub(crate) fn forwards_to_method(module: &str, name: &str) -> bool {
    match module {
        "iter" | "stream" => method_role(name).is_some_and(|role| role.forward),
        // Every `string` member that is a `Str` method with the receiver first.
        // Checked against the VM: each `string.f(s, …) == s.f(…)`.
        "string" => matches!(
            name,
            "len"
                | "is_empty"
                | "lower"
                | "upper"
                | "trim"
                | "reverse"
                | "repeat"
                | "starts_with"
                | "ends_with"
                | "contains"
                | "slice"
                | "index_of"
                | "get"
                | "first"
                | "last"
                | "take"
                | "skip"
                | "replace"
                | "split"
                | "chars"
                | "bytes"
                | "byte_at"
        ),
        _ => false,
    }
}

pub(crate) fn method_role(name: &str) -> Option<&'static MethodRow> {
    METHOD_TABLE.iter().find(|row| row.name == name)
}

/// Constant module members (`math.pi`): a member read resolves to the literal
/// value instead of a function ref. Values mirror the stdlib module's
/// `#[stdlib_value]` exports exactly.
pub(crate) fn module_const(module: &str, name: &str) -> Option<(Const, Ty)> {
    match (module, name) {
        ("math", "pi") => Some((Const::F64(std::f64::consts::PI), Ty::F64)),
        ("math", "e") => Some((Const::F64(std::f64::consts::E), Ty::F64)),
        ("math", "inf") => Some((Const::F64(f64::INFINITY), Ty::F64)),
        ("math", "nan") => Some((Const::F64(f64::NAN), Ty::F64)),
        ("math", "max_int") => Some((Const::I64(i64::MAX), Ty::I64)),
        ("math", "min_int") => Some((Const::I64(i64::MIN), Ty::I64)),
        ("math", "max_float") => Some((Const::F64(f64::MAX), Ty::F64)),
        ("math", "epsilon") => Some((Const::F64(f64::EPSILON), Ty::F64)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `path` members that lower natively, and the two that deliberately do
    /// not.
    ///
    /// This is a table test rather than a differential for a reason worth
    /// stating: **the differential cannot see this**. Delete a row below and
    /// `path_members_answer_the_same_on_both_ends` still passes — the call falls
    /// to the hybrid bridge, prints the same answer, and is merely ~3x slower.
    /// Only a check of the table itself (or `scripts/aot_coverage.sh`, which
    /// pins fallback off) notices.
    ///
    /// `join` and `normalize` are absent on purpose. `join` is variadic, which
    /// the fixed-arity ABI cannot express. `normalize` is lexical path
    /// cleaning, which `std::path` does not do — lowering it means *copying*
    /// the VM's component loop into `lkrt`, giving one rule that has already
    /// carried two bugs a second place to drift. lkrt's discipline is to share
    /// the crate underneath (`std::path`, as it already shares base64/hex/
    /// chrono), never to re-type LK-level logic. Both stay on the bridge until
    /// there is a shared implementation to point at.
    /// Every carrier of a `Bytes | String` member has a row.
    ///
    /// One member, two argument types, and for a long time only the first one
    /// written had a row — so `base64.encode(text)` ran native and
    /// `base64.encode(bytes)` ran on the bridge, which nothing reported. Same
    /// shape the list methods had across their carriers.
    #[test]
    fn both_carriers_of_every_bytes_or_string_member_lower() {
        for (module, member) in [
            ("hash", "sha256"),
            ("hash", "sha1"),
            ("hash", "crc32"),
            ("hash", "fnv64"),
            ("base64", "encode"),
            ("hex", "encode"),
        ] {
            for want in [Ty::Str, Ty::Bytes] {
                assert!(
                    module_call_abi_rows(module, member).any(|row| row.args == [want]),
                    "{module}.{member} has no row for its {want:?} carrier — that carrier \
                     silently falls back to the hybrid bridge"
                );
            }
        }
    }

    /// The `fs` surface lowers, and `metadata` is the one member that does not.
    ///
    /// Everything here is a call the bridge would answer identically, so the
    /// differential is blind to losing any of it — and half of these rows were
    /// missing for exactly that reason, with the lkrt side already written.
    ///
    /// `fs.metadata` answers a four-key `Map`, which needed the map carrier
    /// (`Ty::MapStrDyn`) and the mirrored construction — a map's iteration
    /// order is what `println` prints, so handing back a natively-built map is
    /// only correct if it rehashes the way the VM's does.
    #[test]
    fn the_fs_module_lowers_its_scalar_members() {
        for member in [
            "read_to_string",
            "write",
            "append",
            "read_dir",
            "canonicalize",
            "exists",
            "is_file",
            "is_dir",
            "create_dir",
            "create_dir_all",
            "remove_file",
            "remove_dir",
            "remove_dir_all",
            "rename",
            "copy",
            "temp_dir",
            "metadata",
        ] {
            assert!(
                module_call_abi_rows("fs", member).next().is_some(),
                "fs.{member} lost its native lowering — the bridge answers the same, so nothing \
                 else reports this"
            );
        }
    }

    #[test]
    fn the_path_module_lowers_exactly_its_fixed_arity_members() {
        for member in [
            "parent",
            "file_name",
            "file_stem",
            "extension",
            "with_extension",
            "is_absolute",
            "components",
            "sep",
            "delimiter",
        ] {
            assert!(
                module_call_abi_rows("path", member).next().is_some(),
                "path.{member} lost its native lowering — it now runs on the hybrid bridge, \
                 which no differential test can detect"
            );
        }
        for member in ["join", "normalize"] {
            assert!(
                module_call_abi_rows("path", member).next().is_none(),
                "path.{member} gained a native lowering; if that is intended, say here what \
                 it shares its implementation with"
            );
        }
    }
}
