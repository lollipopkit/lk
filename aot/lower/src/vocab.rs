use super::*;

/// Runtime builtins recognized from `GetGlobal` by name and lowered natively at
/// their `Call` sites. A register holding one of these carries no SSA value:
/// any use other than a call rejects (reads find the register undefined).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Builtin {
    /// `volatile_read_uN(ptr)` / `volatile_write_uN(ptr, value)`.
    ///
    /// Width rides in the variant because that is where the source puts it —
    /// the compiler cannot ask the type checker for a pointee type, which is
    /// why these are intrinsics rather than `*p` syntax.
    /// `cpu_*` — barriers, interrupt masking, wait-for-interrupt, and the
    /// system-control instructions. The payload is the ABI entry name under the
    /// `cpu` module, and *only* that: how many arguments the entry takes and
    /// whether it produces a value are read back out of the ABI table at
    /// lowering time.
    ///
    /// Carrying the arity here as well is what this used to do, alongside a
    /// hard-coded list of the entries that return something. Both were copies
    /// of what the table already says, and a copy of a signature is the shape
    /// this repo has been bitten by: an entry whose arity disagreed would lower
    /// a call with the wrong number of arguments, and one missing from the
    /// returns-a-value list would have its result overwritten with nil.
    Cpu(&'static str),
    VolatileRead(u8),
    VolatileWrite(u8),
    /// `port_in_uN(port)` / `port_out_uN(port, value)` — x86 port I/O.
    PortIn(u8),
    PortOut(u8),
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
    /// `__lk_shl(l, r)` / `__lk_shr(l, r)` — the `<<`/`>>` desugars. Unlike the
    /// other bitwise operators these do not lower to a machine instruction:
    /// the shift amount has to be range-checked, and the check lives in
    /// `lkrt`'s `i64_sh*_checked` so the VM and the native build raise the same
    /// error rather than one masking where the other refuses.
    // TODO: inline the shift with a cold branch to the raise, once codegen can
    // build blocks mid-instruction; a call per shift is the price of the check.
    Shl,
    Shr,
    /// `__lk_shr_u(l, r)` — the compiler picks this when the left operand is a
    /// `u64`, where an arithmetic shift would replicate a bit that is part of
    /// the value rather than its sign.
    ShrU,
    /// `__lk_lt_u` / `__lk_div_u` / `__lk_mod_u` — the unsigned forms the
    /// compiler picks when both operands are proven `u64`.
    LtU,
    DivU,
    ModU,
    /// `__lk_u64_to_float(x)` — the unsigned read of the carrier as a float.
    U64ToFloat,
    /// `__lk_u64_str(x)` — the unsigned read of the carrier as its decimal
    /// string. Inserted by the compiler at the sites that *render* a value
    /// (a `println` argument, a template-string part) rather than compute with
    /// it, because that is the last place the width is still known.
    U64Str,
    /// `symbol_address("name")` — the address of an `#[export]`ed function, and
    /// `call_address_2(addr, a, b)` — a call through one. Together they are
    /// what a driver table is made of: an array of function pointers, indexed
    /// by device or by window, instead of an `if` chain edited for every new
    /// entry. The name must be a literal, because a relocation is a name at
    /// link time and there is nothing to look one up in at run time.
    SymbolAddress,
    CallAddress2,
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
    /// A user function value (`LoadFunction`), with the function it names.
    ///
    /// Two consumers. The compiler's `SetGlobal` storage of a top-level `fn`
    /// declaration, which is a no-op natively. And a `Call` through the
    /// register, which is a direct call the bytecode could not spell that way:
    /// `CallDirect` names its target in a byte, so a module whose 256th
    /// function calls its 257th gets `LoadFunction` + `Call` instead. That used
    /// to reject, which made 256 functions a *native* ceiling as well as a
    /// bytecode one — reached the ordinary way, by a program with a lot of
    /// drivers. The index is what makes the call lowerable; it is the same
    /// devirtualization `Lambda` already gets.
    UserFn(u32),
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
    /// The *enclosing* function's `k`th capture, captured onward.
    ///
    /// A closure nested in a closure (`|v| { let inner = |w| { total = total +
    /// w; }; … }`) captures what its parent captured. The parent holds it as a
    /// capture parameter, not as a cell of its own, so there was nothing for
    /// `Cell(cid)` to name and the whole program fell back.
    ///
    /// When the parent's capture is already a runtime cell (`Ty::Cell`) the
    /// pointer passes straight through — parent and child share one cell, which
    /// is exactly the VM's semantics. When it is not, the child's need for one
    /// propagates up: the call site records it against the parent and retries,
    /// so `SigInfer::cell_captures` reaches a fixpoint over the whole chain.
    CellParam(usize),
    /// A capture whose whole meaning is a lowering-time reference (a lambda, a
    /// named function): nothing to pass, so the slot carries a dead `0` and the
    /// callee reads [`SigInfer::ref_captures`].
    StaticRef,
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
    /// A `try` region, collapsed into one exit.
    ///
    /// The body's instructions are not part of this function: they were
    /// outlined into a function of their own, because Cranelift cannot emit
    /// `setjmp` — a call that returns twice has no place in its SSA or its
    /// register allocator. What is left here is a call whose *outcome* is a
    /// flag, and this exit is the branch on it: fall through when the body
    /// returned, into the handler when it raised.
    TryRegion {
        /// The function the body became.
        body: u32,
        /// The register the handler reads the caught value from.
        catch_reg: u8,
        handler: usize,
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
