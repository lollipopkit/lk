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
    ModuleRow {
        name: "std",
        bare_global: true,
        submodule_of: None,
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
    // `encoding`/`net` submodules (the parents themselves have no typed
    // members — only the submodule objects bind).
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
        name: "socket",
        bare_global: false,
        submodule_of: Some("net"),
    },
    ModuleRow {
        name: "tcp",
        bare_global: false,
        submodule_of: Some("net"),
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
    // Sorted entry names as List<str> (the VM's exact shape).
    abi_row(
        "fs",
        "read_dir",
        AbiRef::new("fs", "read_dir_list"),
        &[Ty::Str],
        Ty::ListStr,
    ),
    abi_row("fs", "exists", AbiRef::new("fs", "exists"), &[Ty::Str], Ty::Bool),
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
    abi_row("path", "sep", AbiRef::new("path", "sep"), &[], Ty::Str),
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
    abi_row(
        "string",
        "capitalize",
        AbiRef::new("str", "capitalize"),
        &[Ty::Str],
        Ty::Str,
    ),
    abi_row("string", "title", AbiRef::new("str", "title"), &[Ty::Str], Ty::Str),
    // Native channels/goroutines (plan H): channel/task values are i64
    // ids; blocking semantics + raises live in lkrt.
    abi_row("chan", "close", AbiRef::new("chan", "close"), &[Ty::I64], Ty::Nil),
    abi_row("chan", "len", AbiRef::new("chan", "len"), &[Ty::I64], Ty::I64),
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
    abi_row("task", "await", AbiRef::new("rt", "task_await"), &[Ty::I64], Ty::Dyn),
    // `encoding` submodules (VM `de.rs` mirrored in lkrt).
    abi_row("json", "parse", AbiRef::new("json", "parse"), &[Ty::Str], Ty::Dyn),
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
    abi_row("tcp", "read", AbiRef::new("tcp", "read"), &[Ty::I64, Ty::I64], Ty::I64),
    abi_row("tcp", "close", AbiRef::new("tcp", "close"), &[Ty::I64], Ty::I64),
    abi_row(
        "bytes",
        "to_string_utf8",
        AbiRef::new("bytes", "to_string_utf8"),
        &[Ty::I64],
        Ty::Str,
    ),
];

pub(crate) fn module_call_abi(module: &str, name: &str) -> Option<(AbiRef, &'static [Ty], Ty)> {
    MODULE_ABI
        .iter()
        .find(|row| row.module == module && row.member == name)
        .map(|row| (row.abi, row.args, row.ret))
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
