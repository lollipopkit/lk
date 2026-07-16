use super::*;

/// Runtime builtins recognized from `GetGlobal` by name and lowered natively at
/// their `Call` sites. A register holding one of these carries no SSA value:
/// any use other than a call rejects (reads find the register undefined).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Builtin {
    Println,
    Print,
    Assert,
    AssertEq,
    AssertNe,
    Panic,
    Typeof,
    /// `__lk_call_method(receiver, name, args_list)` — the compiler's generic
    /// method-dispatch entry; lowered per (receiver type, method name).
    CallMethod,
    /// `Set()` / `Set(list)` — the VM's set constructor builtin.
    SetCtor,
    /// `try$call(closure)` — the try/catch desugar's protected call.
    TryCall,
    /// `error(v)` — raises a first-class error value (`rt.raise_dyn`).
    ErrorRaise,
    /// `chan(capacity[, type])` — a native channel (its `i64` id).
    ChanNew,
    /// `send(c, v)` — blocking, deep-copy, raises on closed.
    ChanSend,
    /// `recv(c)` — blocking, raises once closed and drained.
    ChanRecv,
    /// `spawn(closure)` / the `go` desugar — a goroutine OS thread.
    Spawn,
    /// `__lk_merge_fields(base, overlay)` — the struct-update desugar's
    /// field merge (`P { ..base, k: v }`); the result is a fresh map.
    MergeFields,
    /// `__lk_make_struct(name, fields)` — the struct-update desugar's
    /// object constructor: a fresh field copy + struct provenance.
    MakeStruct,
    /// `__lk_bit_and(l, r)` / `__lk_bit_or(l, r)` / `__lk_bit_not(v)` — the
    /// `&`/`|`/`~` operator desugars (Int-only in the VM; other argument
    /// types reject and fall back to its loud error).
    BitAnd,
    BitOr,
    BitNot,
    /// `select$block(types, chans, values, guards, has_default)`.
    SelectBlock,
}

/// What a register loaded from the global table refers to. Like [`Builtin`],
/// none of these carry an SSA value; only the recognized consumption patterns
/// lower, everything else finds the register undefined and rejects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GlobalRef {
    Builtin(Builtin),
    /// A stdlib module object (`use os;` → `GetGlobal "os"`). Its only
    /// supported consumer is a constant-name member read (`GetIndex` with a
    /// constant string key), which produces [`GlobalRef::ModuleFn`].
    Module(String),
    /// A member function resolved from `module.name`, callable when
    /// [`module_call_abi`] maps it to a typed lkrt ABI entry.
    ModuleFn(String, String),
    /// A compile-time-bundled file module (`use "path"` → `GetGlobal` of the
    /// file-stem binding); the payload indexes `SigInfer::imports.bundles`.
    /// Its only consumer is a constant-name member read, which resolves to
    /// [`GlobalRef::Lambda`] of the merged function.
    UserModule(usize),
    /// A user function value (`LoadFunction`); its only supported consumer is
    /// the compiler's `SetGlobal` storage of top-level `fn` declarations
    /// (direct calls address the callee by index instead).
    UserFn,
    /// A capture-free closure (`MakeClosure` with `capture_count == 0`) — a
    /// statically known function reference. Supported consumers: an indirect
    /// `Call` through the register (lowered as a direct call) and the entry
    /// prefix's `SetGlobal` storage of a top-level `let f = |x| …` (readable
    /// back via `GetGlobal` when the slot is written exactly once).
    Lambda(u32),
    /// A capturing closure with its tracked environment: each capture is a
    /// shared mutable cell (resolved to the cell's *current* value at each
    /// call site — the VM's cell indirection evaluated statically) or a direct
    /// value. The resolved values become hidden trailing call arguments, and
    /// the lambda body reads them as extra parameters. Only an indirect
    /// `Call` through the tracked register is supported (no global storage —
    /// the environment is per-creation-site).
    Closure(u32, Vec<ClosureCapture>),
    /// An upvalue cell (`LoadHeapConst` of `UpvalCell`): the compiler's shared
    /// mutable box for captured locals. Its content is tracked per block in
    /// a virtual SSA slot; the handle itself never materializes.
    Cell(u32),
    /// Inside a lambda body: `LoadCapture k` yields the k-th captured cell;
    /// `LoadCellVal` through it reads the hidden capture parameter.
    CellParam(usize),
    /// A `NewList` argument pack: the compiler boxes method-call arguments
    /// into a list; the lowering keeps the raw elements so method dispatch can
    /// consume them without materializing a runtime list.
    ArgList(Vec<(ValueId, Ty)>),
}

/// The statically known identity of a lambda passed as an argument: the
/// target function plus its capture count (a capturing closure's *environment
/// values* are runtime data — hidden trailing arguments — and stay out of the
/// identity, so one clone serves every environment of the same lambda).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct LambdaIdentity {
    pub(crate) fidx: u32,
    pub(crate) captures: u16,
}

/// One capture of a *returned* closure, expressed in caller terms: the
/// callee's k-th parameter value (i.e. the caller's argument). A returned
/// closure whose environment reduces entirely to parameters lets the call
/// site construct the closure ref statically — the effect-free callee body
/// is never emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RetCaptureSrc {
    Param(usize),
}

/// One captured slot of a [`GlobalRef::Closure`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ClosureCapture {
    /// A shared mutable cell, resolved at each call site.
    Cell(u32),
    /// A direct by-value capture.
    Value(ValueId, Ty),
}

/// One piece of a `print`/`println` output line, assembled at lower time from
/// the (constant) format string and the call arguments.
pub(crate) enum PrintPart {
    Lit(String),
    Val(ValueId, Ty),
}

/// The right-hand side of a fused compare-and-branch: a register or an immediate.
#[derive(Debug, Clone, Copy)]
pub(crate) enum FusedRhs {
    Imm(i64),
    Reg(u8),
}

/// A decoded basic-block terminator over bytecode pc targets.
#[derive(Debug, Clone, Copy)]
pub(crate) enum Exit {
    Ret(Option<u8>),
    Jump(usize),
    Cond {
        cond: u8,
        then_pc: usize,
        else_pc: usize,
    },
    /// Fused `TestXxxInt(I)` + trailing `Jmp`: branch to `taken` when the compare
    /// (negated iff `!jump_when`) holds, else `fallthrough`. Consumes the `Jmp`.
    FusedCmp {
        reg_a: u8,
        rhs: FusedRhs,
        op: CmpOp,
        jump_when: bool,
        taken: usize,
        fallthrough: usize,
    },
    /// `ForLoopI`: increment the index register by the step register, then
    /// branch back to `taken` (the loop head) while the range condition holds,
    /// else fall through. Direction/inclusivity come from the compiler's
    /// for-loop fact (required — there is no fact-less execution path).
    ForLoop {
        index_reg: u8,
        end_reg: u8,
        step_reg: u8,
        inclusive: bool,
        positive_step: bool,
        taken: usize,
        fallthrough: usize,
    },
    /// Fused `TestEqIntI2` + trailing `Jmp`: `r_a == imm_a && r_b == imm_b`
    /// falls through, anything else branches to `taken`. Consumes the `Jmp`.
    FusedCmp2 {
        reg_a: u8,
        imm_a: i64,
        reg_b: u8,
        imm_b: i64,
        taken: usize,
        fallthrough: usize,
    },
    /// Fused `BrMod{Eq,Ne}ZeroIntI4`: branch to `taken` when `r_a % divisor <op> 0`
    /// (`op` is `Eq` for the `Eq`-zero form, `Ne` otherwise), else `fallthrough`. The
    /// modulo goes through the guarded helper (aborts on a zero divisor, matching the
    /// VM's fatal error).
    FusedModZero {
        reg_a: u8,
        divisor: i64,
        op: CmpOp,
        taken: usize,
        fallthrough: usize,
    },
    /// `BrNil`/`BrNotNil`: branch to `taken` when `r_a` is nil (`jump_when_nil`) or
    /// not-nil, else `fallthrough`. Resolved by the operand's static type: a `Maybe`
    /// tests its present bit; a definitely-non-nil scalar / `Nil` folds to a constant.
    NilBranch {
        reg_a: u8,
        jump_when_nil: bool,
        taken: usize,
        fallthrough: usize,
    },
}
