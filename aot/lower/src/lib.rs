//! `lk-aot-lower` — the bytecode → typed MIR bridge.
//!
//! This is the *total capability predicate* of the redesign: [`lower`] turns a
//! bytecode [`ModuleArtifact`] into an [`MirModule`] or returns a precise
//! [`Unsupported`] reason. Nothing downstream (`lk-aot-codegen`) can fail, because
//! anything not expressible in the closed MIR type set is rejected right here.
//!
//! Scope today (the growing strangler slice):
//! - A single, parameter-free, capture-free entry function, plus **direct calls**
//!   to other `(i64, …) -> i64` functions (recursion included).
//! - Scalar values: int/float/bool/nil constants; register moves. Arithmetic and
//!   comparisons **dispatch on operand type** (two ints → integer op; any float →
//!   coerce ints and use the float op), matching the VM's runtime numeric dispatch.
//! - **Full acyclic + reducible-loop control flow via on-demand SSA construction**
//!   (Braun et al.): `Test`/`BrTrue`/`BrFalse`/`Jmp` and fused `TestXxxInt(I)`+`Jmp`
//!   become MIR blocks with `CondBr`/`Br`. Registers live across a merge or a loop
//!   back-edge become SSA phi params (int or float) constructed on demand, with
//!   incomplete phis in unsealed (loop-header) blocks filled once the back-edge
//!   predecessor is lowered.
//! - **Growable `List<i64>` / `List<f64>` handles** (Phase 2 handle-ification):
//!   materialize constant list literals; `.len()`; and **provably in-range constant
//!   indexing** (`GetList`). Dynamic/out-of-range indexing (which is `Maybe<Int>` in
//!   the VM) still rejects to avoid a nil-semantics divergence.
//!
//! Anything outside this subset (dynamic indexing, list mutation, maps, closures,
//! non-`i64` function ABIs, …) returns `Unsupported`; the caller falls back to the
//! legacy backend. See `docs/llvm/aot-redesign.md` §7/§9.5.
//!
//! (Trivial-phi elimination is intentionally omitted: the constructed SSA is
//! correct but not minimal — a self-referential loop phi is valid LLVM and is left
//! for `opt` / a later cleanup pass.)

use std::collections::BTreeMap;

use lk_aot_mir::{
    AbiRef, Block, BlockId, CmpOp, Const, FloatBinOp, FuncId, GlobalId, Inst, IntBinOp, MirFunction, MirModule, Term,
    Ty, ValueId, VmFunction,
};
use lk_core::vm::{
    ConstHeapValueData, ConstRuntimeValueData, FunctionData, Instr, ModuleArtifact, Opcode, RuntimeMapKeyData,
};

/// Why a bytecode artifact cannot (yet) be lowered to MIR.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unsupported {
    NoEntry,
    EntryHasParams(u16),
    EntryHasCaptures(u16),
    BadInstr {
        pc: usize,
    },
    Opcode {
        pc: usize,
        op: Opcode,
    },
    BadConst {
        pc: usize,
    },
    /// A register (or virtual cell slot) was read with no reaching definition
    /// on any predecessor path.
    UndefinedOperand {
        pc: usize,
        reg: usize,
    },
    /// An operand had the wrong type for the operation.
    TypeMismatch {
        pc: usize,
    },
    NoReturn,
    /// A branch condition register was not a `Bool` (int-truthiness not yet lowered).
    NonBoolCondition {
        pc: usize,
    },
    /// Two returns disagree on the value type.
    ReturnTypeConflict,
    /// A branch/jump target fell outside the code.
    BadTarget {
        pc: usize,
    },
}

impl Unsupported {
    /// A user-facing explanation of why the program is not natively lowerable
    /// (yet). Every enum variant maps to one sentence here, so the capability
    /// boundary is testable and documentable (RFC aot-redesign §3.5).
    pub fn reason(&self) -> String {
        match self {
            Unsupported::NoEntry => "the module has no entry function".to_string(),
            Unsupported::EntryHasParams(n) => format!("the entry function takes {n} parameter(s)"),
            Unsupported::EntryHasCaptures(n) => format!("the entry function captures {n} value(s)"),
            Unsupported::BadInstr { pc } => format!("undecodable instruction at pc {pc}"),
            Unsupported::Opcode { pc, op } => {
                format!("opcode {op:?} (at pc {pc}) is not natively lowerable yet")
            }
            Unsupported::BadConst { pc } => format!("unsupported constant operand at pc {pc}"),
            Unsupported::UndefinedOperand { pc, reg } => {
                format!("register r{reg} is read at pc {pc} before any definition")
            }
            Unsupported::TypeMismatch { pc } => {
                format!("an operand at pc {pc} has a type outside the natively lowerable subset")
            }
            Unsupported::NoReturn => "the entry function never returns".to_string(),
            Unsupported::NonBoolCondition { pc } => {
                format!("the branch condition at pc {pc} is not a bool")
            }
            Unsupported::ReturnTypeConflict => "returns disagree on the value type".to_string(),
            Unsupported::BadTarget { pc } => format!("a branch at pc {pc} targets an out-of-range pc"),
        }
    }
}

impl std::fmt::Display for Unsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.reason())
    }
}

type Reg = (ValueId, Ty);

/// Runtime builtins recognized from `GetGlobal` by name and lowered natively at
/// their `Call` sites. A register holding one of these carries no SSA value:
/// any use other than a call rejects (reads find the register undefined).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Builtin {
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
}

/// What a register loaded from the global table refers to. Like [`Builtin`],
/// none of these carry an SSA value; only the recognized consumption patterns
/// lower, everything else finds the register undefined and rejects.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GlobalRef {
    Builtin(Builtin),
    /// A stdlib module object (`use os;` → `GetGlobal "os"`). Its only
    /// supported consumer is a constant-name member read (`GetIndex` with a
    /// constant string key), which produces [`GlobalRef::ModuleFn`].
    Module(String),
    /// A member function resolved from `module.name`, callable when
    /// [`module_call_abi`] maps it to a typed lkrt ABI entry.
    ModuleFn(String, String),
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
struct LambdaIdentity {
    fidx: u32,
    captures: u16,
}

/// One capture of a *returned* closure, expressed in caller terms: the
/// callee's k-th parameter value (i.e. the caller's argument). A returned
/// closure whose environment reduces entirely to parameters lets the call
/// site construct the closure ref statically — the effect-free callee body
/// is never emitted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RetCaptureSrc {
    Param(usize),
}

/// One captured slot of a [`GlobalRef::Closure`].
#[derive(Debug, Clone, PartialEq, Eq)]
enum ClosureCapture {
    /// A shared mutable cell, resolved at each call site.
    Cell(u32),
    /// A direct by-value capture.
    Value(ValueId, Ty),
}

/// Global names treated as stdlib module objects when read via `GetGlobal`.
/// Only modules with at least one [`module_call_abi`] mapping belong here.
const MODULE_GLOBALS: &[&str] = &["os", "time", "env", "math", "fs", "process", "datetime", "std"];

/// Maps a `module.method` call to its typed lkrt ABI entry: the `AbiRef`, the
/// exact positional argument types, and the return type. `None` means the
/// member is not natively lowerable (yet) and the program falls back.
/// (`math.floor` dispatches on its argument type in [`lower_module_call`].)
///
/// Every entry must be VM-exact: same value semantics *and* the same display
/// (the differential corpora compare stdout byte-for-byte).
fn module_call_abi(module: &str, name: &str) -> Option<(AbiRef, &'static [Ty], Ty)> {
    match (module, name) {
        // Monotonic in-process seconds (f64) — both sides anchor to first use.
        ("os", "clock") => Some((AbiRef::new("os", "clock"), &[], Ty::F64)),
        // Unix epoch milliseconds.
        ("os", "epoch") => Some((AbiRef::new("os", "epoch"), &[], Ty::I64)),
        // Monotonic milliseconds / sleep-for-milliseconds.
        ("time", "now") => Some((AbiRef::new("time", "now"), &[], Ty::I64)),
        ("time", "sleep") => Some((AbiRef::new("time", "sleep"), &[Ty::I64], Ty::Nil)),
        // Environment lookup with a default; both sides return an owned string.
        ("env", "get_or") => Some((AbiRef::new("env", "get_or"), &[Ty::Str, Ty::Str], Ty::Str)),
        // System info strings (allocated per call, arena-owned).
        ("os", "hostname") => Some((AbiRef::new("os", "hostname"), &[], Ty::Str)),
        ("os", "arch") => Some((AbiRef::new("os", "arch"), &[], Ty::Str)),
        ("os", "os") => Some((AbiRef::new("os", "name"), &[], Ty::Str)),
        ("process", "cwd") => Some((AbiRef::new("process", "cwd"), &[], Ty::Str)),
        ("fs", "temp_dir") => Some((AbiRef::new("fs", "temp_dir"), &[], Ty::Str)),
        // Sorted entry names as List<str> (the VM's exact shape).
        ("fs", "read_dir") => Some((AbiRef::new("fs", "read_dir_list"), &[Ty::Str], Ty::ListStr)),
        // chrono-backed datetime (byte-identical to the stdlib module).
        ("datetime", "now") => Some((AbiRef::new("datetime", "now"), &[], Ty::I64)),
        ("datetime", "format") => Some((AbiRef::new("datetime", "format"), &[Ty::I64, Ty::Str], Ty::Str)),
        ("datetime", "parse") => Some((AbiRef::new("datetime", "parse"), &[Ty::Str, Ty::Str], Ty::I64)),
        ("datetime", "day_of_week") => Some((AbiRef::new("datetime", "day_of_week"), &[Ty::I64], Ty::I64)),
        ("datetime", "day_of_year") => Some((AbiRef::new("datetime", "day_of_year"), &[Ty::I64], Ty::I64)),
        // Float-typed math (Number args f64-promote at the call site). `sqrt`
        // aborts on a negative argument (the stdlib module's loud error).
        ("math", "sqrt") => Some((AbiRef::new("math", "sqrt"), &[Ty::F64], Ty::F64)),
        ("math", "sin") => Some((AbiRef::new("math", "sin"), &[Ty::F64], Ty::F64)),
        ("math", "cos") => Some((AbiRef::new("math", "cos"), &[Ty::F64], Ty::F64)),
        ("math", "exp") => Some((AbiRef::new("math", "exp"), &[Ty::F64], Ty::F64)),
        ("math", "pow") => Some((AbiRef::new("math", "pow"), &[Ty::F64, Ty::F64], Ty::F64)),
        _ => None,
    }
}

/// Constant module members (`math.pi`): a member read resolves to the literal
/// value instead of a function ref. Values mirror the stdlib module's
/// `#[stdlib_value]` exports exactly.
fn module_const(module: &str, name: &str) -> Option<(Const, Ty)> {
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

/// One piece of a `print`/`println` output line, assembled at lower time from
/// the (constant) format string and the call arguments.
enum PrintPart {
    Lit(String),
    Val(ValueId, Ty),
}

/// The right-hand side of a fused compare-and-branch: a register or an immediate.
#[derive(Debug, Clone, Copy)]
enum FusedRhs {
    Imm(i64),
    Reg(u8),
}

/// A decoded basic-block terminator over bytecode pc targets.
#[derive(Debug, Clone, Copy)]
enum Exit {
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

/// Inferred function signatures, refined to a fixpoint before the final lowering.
///
/// User functions use a monomorphic `(params...) -> ret` native ABI. Neither the
/// parameter types nor the return type are in the bytecode, so both are inferred:
///  - `ret_types[f]` — from `f`'s return value type (returns can chain: `f` returns
///    `g()`), so it iterates.
///  - `param_obs[f][i]` — the argument type observed at `f`'s `CallDirect` sites. If
///    every site agrees, that is the parameter type; if two sites disagree, `f` is
///    polymorphic (`conflict`) and cannot be monomorphized, so the whole module
///    falls back rather than miscompiling one of the call sites.
struct SigInfer {
    param_obs: Vec<Vec<Option<Ty>>>,
    ret_types: Vec<Ty>,
    conflict: bool,
    /// Per module-global slot: the scalar type every `SetGlobal` writes (a
    /// mixed-type global marks `conflict`, rejecting the module rather than
    /// miscompiling one of the writes).
    global_tys: Vec<Option<Ty>>,
    /// Slots written by the entry function *before* any control flow or user
    /// call. Only these may be read via `GetGlobal`: the VM initializes
    /// globals to nil while native storage zero-initializes, so a read that
    /// could observe the pre-first-write value must reject.
    initialized_globals: Vec<bool>,
    /// Slots holding a top-level capture-free closure (`let f = |x| …`):
    /// assigned exactly once, in the entry prefix, from a zero-capture
    /// `MakeClosure`. Reading such a slot yields [`GlobalRef::Lambda`].
    lambda_globals: Vec<Option<u32>>,
    /// `lambda_params[f][i]` — this function's i-th parameter is an *erased*
    /// lambda with a statically known identity: the callee seeds the register
    /// with a `GlobalRef::Lambda`/`Closure` instead of binding a value, so
    /// indirect calls through it devirtualize. A capturing identity adds
    /// hidden environment parameters (after the visible ones, before the
    /// callee's own captures). Set on clone materialization.
    lambda_params: Vec<Vec<Option<LambdaIdentity>>>,
    /// Clone specialization table: `(original fn, lambda identity per param)`
    /// → the specialized clone's id. Call sites passing lambdas retarget to
    /// the clone whose identity vector matches, so *different* lambdas at the
    /// same parameter get independent clones instead of a conflict.
    specializations: std::collections::HashMap<(u32, Vec<Option<LambdaIdentity>>), u32>,
    /// Clones queued during a pass (original fn ids, in id-assignment order),
    /// materialized into the working function list between passes.
    pending_clones: Vec<u32>,
    /// Original functions that have at least one specialized (lambda-passing)
    /// call site. If such a function also has a plain call site
    /// (`plain_called`), the program is polymorphic over functions vs values —
    /// reject. Otherwise the original body is skipped (all callers use clones).
    specialized: Vec<bool>,
    /// Original functions with at least one all-plain call site.
    plain_called: Vec<bool>,
    /// `ret_closures[f]` — this function's single return is a closure whose
    /// captures all map to its parameters: `(lambda fidx, capture sources)`.
    /// Call sites consume the summary (the result register is seeded with the
    /// closure ref, no call emitted); the pure body is never emitted.
    ret_closures: Vec<Option<(u32, Vec<RetCaptureSrc>)>>,
    /// Functions whose returns disagreed with a recorded summary — a poisoned
    /// function never records again and rejects on lowering instead.
    ret_closure_poisoned: Vec<bool>,
    /// Diagnostic names for the mutable-global table (slot-indexed).
    global_names: Vec<String>,
    /// Final compact `slot → gvar` numbering, built once signatures converge
    /// (empty during the fixpoint passes, whose emitted MIR is discarded).
    gvar_of: std::collections::HashMap<u16, u32>,
    /// Tier 1 hybrid: functions whose bodies did not lower but whose call
    /// sites bridge into the embedded VM (`fidx → scalar marshaling types`).
    /// Empty during the fixpoint; filled between the failing final pass and
    /// its hybrid retry (`docs/llvm/tier1-hybrid.md`).
    vm_functions: std::collections::HashMap<u32, Vec<Ty>>,
}

impl SigInfer {
    fn param_ty(&self, func: usize, i: usize) -> Ty {
        self.param_obs[func].get(i).copied().flatten().unwrap_or(Ty::I64)
    }

    fn gvar(&self, slot: u16) -> u32 {
        self.gvar_of.get(&slot).copied().unwrap_or(u32::from(slot))
    }
}

pub fn lower(artifact: &ModuleArtifact) -> Result<MirModule, Unsupported> {
    // Opt-in until the CLI links the bridge runtime (tier1-hybrid.md sub-step
    // ④): emitting `CallVm` without the lk-api staticlib linked would turn
    // graceful `Unsupported` fallbacks into link errors.
    let hybrid = std::env::var_os("LK_AOT_HYBRID").is_some_and(|value| value != "0");
    lower_with_hybrid(artifact, hybrid)
}

/// [`lower`] with the Tier 1 hybrid mode passed explicitly (tests use this to
/// avoid process-global env mutation): when `hybrid` is set, a reachable
/// non-entry function whose body does not lower can be marked *VM-executed*
/// instead of failing the module, provided it is bridge-eligible (scalar
/// parameters, no captures or lambda machinery, transitively user-global-free
/// — see `docs/llvm/tier1-hybrid.md`).
pub fn lower_with_hybrid(artifact: &ModuleArtifact, hybrid: bool) -> Result<MirModule, Unsupported> {
    let module = &artifact.module;
    if module.functions.is_empty() {
        return Err(Unsupported::NoEntry);
    }
    let n = module.functions.len();

    // Reachability from the entry via `CallDirect`. Functions that are defined but
    // never directly called (e.g. a small helper the front end inlined at every use)
    // are dead for AOT; lowering them would pointlessly fail the whole module if they
    // use a shape we don't support, so we skip them entirely.
    let mut reachable = reachable_functions(module);

    let global_count = module.globals.len();
    let mut sig = SigInfer {
        // Captures are hidden trailing parameters, so a capturing lambda's
        // signature covers `param_count + capture_count` slots.
        param_obs: module
            .functions
            .iter()
            .map(|f| vec![None; f.param_count as usize + f.capture_count as usize])
            .collect(),
        ret_types: vec![Ty::I64; n],
        conflict: false,
        global_tys: vec![None; global_count],
        initialized_globals: prescan_initialized_globals(module, global_count),
        lambda_globals: prescan_lambda_globals(module, global_count),
        lambda_params: module
            .functions
            .iter()
            .map(|f| vec![None; f.param_count as usize])
            .collect(),
        specializations: std::collections::HashMap::new(),
        pending_clones: Vec::new(),
        specialized: vec![false; n],
        plain_called: vec![false; n],
        ret_closures: vec![None; n],
        ret_closure_poisoned: vec![false; n],
        global_names: module.globals.clone(),
        gvar_of: std::collections::HashMap::new(),
        vm_functions: std::collections::HashMap::new(),
    };

    // Working function list: module functions plus lambda-argument clone
    // specializations materialized between fixpoint passes (byte-identical
    // bodies whose `lambda_params` erase the lambda parameters).
    let mut funcs: Vec<FunctionData> = module.functions.to_vec();

    // Fixpoint: re-lower every function, refining inferred parameter/return types
    // (bounded — the scalar lattice converges quickly). Transient failures are
    // tolerated here (a function may not lower until the types it depends on have
    // converged); the final pass below is authoritative and propagates errors.
    let mut passes = 0usize;
    loop {
        let snapshot = (
            sig.param_obs.clone(),
            sig.ret_types.clone(),
            sig.specializations.len(),
            sig.ret_closures.clone(),
        );
        // Call-site facts are re-derived every pass: an argument register
        // that resolves to a closure ref only once a summary lands (e.g. a
        // summarized callee's result) must not leave a stale plain-call mark
        // from the pass before the summary existed. The final pass inherits
        // the converged flags of the last fixpoint pass.
        sig.specialized.iter_mut().for_each(|flag| *flag = false);
        sig.plain_called.iter_mut().for_each(|flag| *flag = false);
        sig.conflict = false;
        for fi in 0..funcs.len() {
            if !reachable[fi] {
                continue;
            }
            // Originals whose call sites all pass lambdas are fully replaced
            // by their clones (their bodies would reject without erasure).
            if sig.specialized.get(fi).copied().unwrap_or(false) {
                continue;
            }
            // Summarized closure-returning functions are consumed at call
            // sites; their pure bodies are never emitted.
            if sig.ret_closures.get(fi).is_some_and(Option::is_some) {
                continue;
            }
            let is_entry = fi as u32 == module.entry;
            let mut scratch = Vec::new();
            if let Ok(mf) = lower_function(
                &funcs[fi],
                &funcs,
                fi as u32,
                module.entry,
                is_entry,
                &mut scratch,
                &module.globals,
                &mut sig,
            ) && !is_entry
            {
                sig.ret_types[fi] = mf.ret;
            }
        }
        // Materialize clones queued during this pass so the next pass lowers
        // them (their `lambda_params` are already in place).
        for orig in std::mem::take(&mut sig.pending_clones) {
            funcs.push(funcs[orig as usize].clone());
            reachable.push(true);
        }
        passes += 1;
        let converged = snapshot
            == (
                sig.param_obs.clone(),
                sig.ret_types.clone(),
                sig.specializations.len(),
                sig.ret_closures.clone(),
            );
        if converged || passes > 2 * funcs.len() + 2 {
            break;
        }
    }
    // A conflict that survives the *converged* pass is real (per-pass resets
    // clear transient marks left before a closure-return summary landed):
    // param-type disagreement or function-vs-value polymorphism.
    if sig.conflict {
        return Err(Unsupported::ReturnTypeConflict);
    }

    // Compact numbering for the mutable globals the fixpoint discovered; the
    // final pass emits `GlobalGet`/`GlobalSet` against these ids.
    let mut mutable_globals: Vec<(String, Ty)> = Vec::new();
    for (slot, ty) in sig.global_tys.clone().into_iter().enumerate() {
        if let Some(ty) = ty {
            sig.gvar_of.insert(slot as u16, mutable_globals.len() as u32);
            let name = sig.global_names.get(slot).cloned().unwrap_or_default();
            mutable_globals.push((name, ty));
        }
    }

    // Final pass with stable signatures: produce the real MIR + interned globals for
    // the reachable functions. Any reachable function outside the subset fails the
    // whole module (caller falls back) — unless `hybrid` is set and every failing
    // function is bridge-eligible, in which case the pass reruns with those
    // functions marked VM-executed (their call sites emit `CallVm`, their bodies
    // are skipped). `FuncId`s keep their original module indices.
    let final_pass = |sig: &mut SigInfer, reach: &[bool]| {
        let mut globals: Vec<String> = Vec::new();
        let mut functions = Vec::with_capacity(funcs.len());
        let mut failures: Vec<(usize, Unsupported)> = Vec::new();
        for fi in 0..funcs.len() {
            if !reach.get(fi).copied().unwrap_or(false) {
                continue;
            }
            if sig.specialized.get(fi).copied().unwrap_or(false) {
                continue;
            }
            if sig.ret_closures.get(fi).is_some_and(Option::is_some) {
                continue;
            }
            if sig.vm_functions.contains_key(&(fi as u32)) {
                continue;
            }
            let is_entry = fi as u32 == module.entry;
            match lower_function(
                &funcs[fi],
                &funcs,
                fi as u32,
                module.entry,
                is_entry,
                &mut globals,
                &module.globals,
                sig,
            ) {
                Ok(function) => functions.push(function),
                Err(err) => failures.push((fi, err)),
            }
        }
        (globals, functions, failures)
    };
    let (mut globals, mut functions, failures) = final_pass(&mut sig, &reachable);
    if !failures.is_empty() {
        let first_error = failures[0].1.clone();
        if !hybrid {
            return Err(first_error);
        }
        // Mark every *eligible* failure VM-executed, then recompute
        // reachability without descending into VM-executed functions: their
        // subtrees (e.g. a try/catch desugar closure with captures) run on the
        // VM from the embedded artifact and need no native body. A failure
        // that stays native-reachable and is not eligible fails the module
        // whole (the current Tier 0 behavior).
        let written = written_global_slots(&funcs);
        for (fi, _) in &failures {
            if let Some(params) = bridge_eligibility(*fi, &funcs, module.entry, &sig, &written) {
                sig.vm_functions.insert(*fi as u32, params);
            }
        }
        let native_reachable = native_reachable_functions(&funcs, module.entry, &sig.vm_functions);
        for (fi, _) in &failures {
            if native_reachable.get(*fi).copied().unwrap_or(false) && !sig.vm_functions.contains_key(&(*fi as u32)) {
                return Err(first_error);
            }
        }
        // Drop VM marks without any native-reachable call site (a callee only
        // ever called from inside the VM needs no bridge signature).
        sig.vm_functions
            .retain(|fidx, _| native_reachable.get(*fidx as usize).copied().unwrap_or(false));
        // Rerun with the VM-executed set in place: callers now emit `CallVm`
        // and leave result registers unbound, so a caller that *uses* a
        // bridged result fails here — results never cross the bridge (v1).
        let (retry_globals, retry_functions, retry_failures) = final_pass(&mut sig, &native_reachable);
        if let Some((_, err)) = retry_failures.into_iter().next() {
            return Err(err);
        }
        globals = retry_globals;
        functions = retry_functions;
    }
    if sig.conflict {
        return Err(Unsupported::ReturnTypeConflict);
    }
    let mut vm_functions: Vec<VmFunction> = sig
        .vm_functions
        .iter()
        .map(|(&fidx, params)| VmFunction {
            id: FuncId(fidx),
            params: params.clone(),
        })
        .collect();
    vm_functions.sort_by_key(|vm_fn| vm_fn.id.0);
    Ok(MirModule {
        abi_version: lk_aot_abi::ABI_VERSION,
        globals,
        mutable_globals,
        vm_functions,
        entry: FuncId(module.entry),
        functions,
    })
}

/// Reachability from the entry over `CallDirect`/`MakeClosure` edges that does
/// **not** descend into VM-executed functions: their bodies (and everything
/// only they reach) run on the embedded VM, so no native lowering is needed.
/// VM-executed functions themselves stay marked (they need native call sites).
fn native_reachable_functions(
    funcs: &[FunctionData],
    entry: u32,
    vm_functions: &std::collections::HashMap<u32, Vec<Ty>>,
) -> Vec<bool> {
    let n = funcs.len();
    let mut reachable = vec![false; n];
    let entry = entry as usize;
    if entry >= n {
        return reachable;
    }
    let mut stack = vec![entry];
    reachable[entry] = true;
    while let Some(fi) = stack.pop() {
        if vm_functions.contains_key(&(fi as u32)) {
            continue;
        }
        for raw in &funcs[fi].code {
            let Ok(instr) = Instr::try_from_raw(*raw) else {
                continue;
            };
            let callee = match instr.opcode() {
                Opcode::CallDirect | Opcode::MakeClosure => instr.b() as usize,
                _ => continue,
            };
            if callee < n && !reachable[callee] {
                reachable[callee] = true;
                stack.push(callee);
            }
        }
    }
    reachable
}

/// Global slots written by a `SetGlobal` anywhere in the module. Native code
/// keeps these in native storage, so a VM-executed function must never read
/// them (the bridge VM's copies would diverge); slots *never* written are
/// runtime-builtin reads, which the bridge seeds identically to a VM run.
fn written_global_slots(funcs: &[FunctionData]) -> std::collections::HashSet<u16> {
    let mut written = std::collections::HashSet::new();
    for func in funcs {
        for raw in &func.code {
            let Ok(instr) = Instr::try_from_raw(*raw) else {
                continue;
            };
            if instr.opcode() == Opcode::SetGlobal {
                written.insert(instr.bx());
            }
        }
    }
    written
}

/// Whether failing function `fi` can run on the bridge VM instead of failing
/// the module (`docs/llvm/tier1-hybrid.md`, v1): not the entry, no captures or
/// lambda-erasure machinery, every parameter observed as one scalar type, and
/// its whole `CallDirect`/`MakeClosure`-reachable subtree writes no globals
/// and reads none that the module writes. Returns the scalar marshaling types.
fn bridge_eligibility(
    fi: usize,
    funcs: &[FunctionData],
    entry: u32,
    sig: &SigInfer,
    written_slots: &std::collections::HashSet<u16>,
) -> Option<Vec<Ty>> {
    if fi as u32 == entry {
        return None;
    }
    let func = funcs.get(fi)?;
    if func.capture_count != 0 {
        return None;
    }
    if sig.specialized.get(fi).copied().unwrap_or(false) {
        return None;
    }
    if sig
        .lambda_params
        .get(fi)
        .is_some_and(|params| params.iter().any(Option::is_some))
    {
        return None;
    }
    if sig.ret_closures.get(fi).is_some_and(Option::is_some) {
        return None;
    }
    let mut params = Vec::with_capacity(func.param_count as usize);
    for i in 0..func.param_count as usize {
        match sig.param_obs.get(fi).and_then(|obs| obs.get(i)).copied().flatten() {
            Some(ty @ (Ty::I64 | Ty::F64 | Ty::Bool | Ty::Str)) => params.push(ty),
            _ => return None,
        }
    }
    let mut visited = vec![false; funcs.len()];
    let mut work = vec![fi];
    visited[fi] = true;
    while let Some(cur) = work.pop() {
        for raw in &funcs[cur].code {
            let Ok(instr) = Instr::try_from_raw(*raw) else {
                return None;
            };
            match instr.opcode() {
                Opcode::SetGlobal => return None,
                Opcode::GetGlobal => {
                    if written_slots.contains(&instr.bx()) {
                        return None;
                    }
                }
                Opcode::CallDirect | Opcode::MakeClosure => {
                    let callee = instr.b() as usize;
                    if callee < funcs.len() && !visited[callee] {
                        visited[callee] = true;
                        work.push(callee);
                    }
                }
                _ => {}
            }
        }
    }
    Some(params)
}

/// Slots the entry function writes before any control flow or user-function
/// call: the linear instruction prefix up to the first branch/jump/return/
/// `CallDirect`/`CallNamed`. Reads of other globals could observe the VM's nil
/// initialization (native storage zero-initializes instead), so only these
/// slots are readable via `GetGlobal`. Runtime-builtin `Call`s (println,
/// os.clock, …) cannot read user globals and do not stop the scan.
/// Finds module-global slots that hold a top-level capture-free closure:
/// written exactly once in the whole module, in the entry prefix (same
/// straight-line region as [`prescan_initialized_globals`]), from the result
/// of a zero-capture `MakeClosure`. Only such slots may resolve to
/// [`GlobalRef::Lambda`] on `GetGlobal` — a slot with any other write could be
/// observed with a different value at runtime.
fn prescan_lambda_globals(module: &lk_core::vm::ModuleData, global_count: usize) -> Vec<Option<u32>> {
    let mut candidates: Vec<Option<u32>> = vec![None; global_count];
    let mut write_counts = vec![0usize; global_count];
    for func in &module.functions {
        for raw in &func.code {
            let Ok(instr) = Instr::try_from_raw(*raw) else {
                break;
            };
            if instr.opcode() == Opcode::SetGlobal
                && let Some(count) = write_counts.get_mut(instr.bx() as usize)
            {
                *count += 1;
            }
        }
    }
    let Some(entry) = module.functions.get(module.entry as usize) else {
        return vec![None; global_count];
    };
    // Register → zero-capture closure function index, tracked through the
    // entry prefix (`Move` propagates, any other write clears).
    let mut lambda_regs: std::collections::HashMap<u8, u32> = std::collections::HashMap::new();
    for raw in &entry.code {
        let Ok(instr) = Instr::try_from_raw(*raw) else {
            break;
        };
        match instr.opcode() {
            Opcode::MakeClosure => {
                let fidx = instr.b() as usize;
                let zero_capture = module.functions.get(fidx).is_some_and(|f| f.capture_count == 0);
                if zero_capture {
                    lambda_regs.insert(instr.a(), fidx as u32);
                } else {
                    lambda_regs.remove(&instr.a());
                }
            }
            Opcode::Move => {
                match lambda_regs.get(&instr.b()).copied() {
                    Some(fidx) => lambda_regs.insert(instr.a(), fidx),
                    None => lambda_regs.remove(&instr.a()),
                };
            }
            Opcode::Move2 => {
                // `a ← b`, then `b ← c` — both destinations must be retracked.
                match lambda_regs.get(&instr.b()).copied() {
                    Some(fidx) => lambda_regs.insert(instr.a(), fidx),
                    None => lambda_regs.remove(&instr.a()),
                };
                match lambda_regs.get(&instr.c()).copied() {
                    Some(fidx) => lambda_regs.insert(instr.b(), fidx),
                    None => lambda_regs.remove(&instr.b()),
                };
            }
            Opcode::SetGlobal => {
                let slot = instr.bx() as usize;
                if let (Some(&fidx), Some(candidate)) = (lambda_regs.get(&instr.a()), candidates.get_mut(slot)) {
                    *candidate = Some(fidx);
                }
            }
            // Same prefix boundary as `prescan_initialized_globals`.
            Opcode::Jmp
            | Opcode::Test
            | Opcode::BrFalse
            | Opcode::BrTrue
            | Opcode::BrNil
            | Opcode::BrNotNil
            | Opcode::BrEqZeroInt
            | Opcode::BrNeZeroInt
            | Opcode::BrEqIntI4
            | Opcode::BrNeIntI4
            | Opcode::BrModEqZeroIntI4
            | Opcode::BrModNeZeroIntI4
            | Opcode::ForLoopI
            | Opcode::Return
            | Opcode::Return0
            | Opcode::Return1
            | Opcode::CallDirect
            | Opcode::CallNamed
            | Opcode::TryBegin
            | Opcode::Raise => break,
            op if op.is_compare_test() => break,
            _ => {
                // Any other write to a tracked register invalidates it. The
                // instruction encodings vary; conservatively clear `a` for
                // every remaining opcode (no tracked pattern writes elsewhere).
                lambda_regs.remove(&instr.a());
            }
        }
    }
    for (candidate, count) in candidates.iter_mut().zip(&write_counts) {
        if *count != 1 {
            *candidate = None;
        }
    }
    candidates
}

fn prescan_initialized_globals(module: &lk_core::vm::ModuleData, global_count: usize) -> Vec<bool> {
    let mut initialized = vec![false; global_count];
    let Some(entry) = module.functions.get(module.entry as usize) else {
        return initialized;
    };
    for raw in &entry.code {
        let Ok(instr) = Instr::try_from_raw(*raw) else {
            break;
        };
        match instr.opcode() {
            Opcode::SetGlobal => {
                if let Some(flag) = initialized.get_mut(instr.bx() as usize) {
                    *flag = true;
                }
            }
            Opcode::Jmp
            | Opcode::Test
            | Opcode::BrFalse
            | Opcode::BrTrue
            | Opcode::BrNil
            | Opcode::BrNotNil
            | Opcode::BrEqZeroInt
            | Opcode::BrNeZeroInt
            | Opcode::BrEqIntI4
            | Opcode::BrNeIntI4
            | Opcode::BrModEqZeroIntI4
            | Opcode::BrModNeZeroIntI4
            | Opcode::ForLoopI
            | Opcode::Return
            | Opcode::Return0
            | Opcode::Return1
            | Opcode::CallDirect
            | Opcode::CallNamed
            | Opcode::TryBegin
            | Opcode::Raise => break,
            op if op.is_compare_test() => break,
            _ => {}
        }
    }
    initialized
}

/// Marks which functions are reachable from the entry by following `CallDirect`
/// edges (a worklist over the static call graph). Unreachable functions are dead for
/// AOT — they are never emitted, so an unsupported shape in dead code cannot fail the
/// module.
fn reachable_functions(module: &lk_core::vm::ModuleData) -> Vec<bool> {
    let n = module.functions.len();
    let mut reachable = vec![false; n];
    let entry = module.entry as usize;
    if entry >= n {
        return reachable;
    }
    let mut stack = vec![entry];
    reachable[entry] = true;
    while let Some(fi) = stack.pop() {
        for raw in &module.functions[fi].code {
            let Ok(instr) = Instr::try_from_raw(*raw) else {
                continue;
            };
            // A `MakeClosure` target is indirectly callable (Lambda/Closure
            // refs), so it must be lowered/emitted too.
            let callee = match instr.opcode() {
                Opcode::CallDirect | Opcode::MakeClosure => instr.b() as usize,
                _ => continue,
            };
            if callee < n && !reachable[callee] {
                reachable[callee] = true;
                stack.push(callee);
            }
        }
    }
    reachable
}

/// Best-effort lookahead to type an **empty** map literal (`{}`), which is otherwise
/// ambiguous between string- and int-keyed. Follows the container register (through
/// `Move`s) to its first keyed use: `SetIndex`/`GetIndex` ⇒ int-keyed (an empty list
/// store would be an out-of-bounds error, so a working `[i]=…` implies a map), while
/// `SetFieldK`/`GetFieldK` ⇒ string-keyed. This only affects *coverage*: a wrong
/// guess makes a later op mismatch and the whole module falls back — never a
/// miscompile. Defaults to string-keyed if no keyed use is seen.
fn empty_map_is_int_keyed(code: &[u32], start_pc: usize, dst_reg: u8) -> bool {
    let mut regs = std::collections::HashSet::new();
    regs.insert(dst_reg);
    // Registers most recently written by a string producer: an index through
    // one of these means the map is string-keyed. A wrong guess only costs a
    // fallback (the typed lowering rejects the mismatch), never a miscompile.
    let mut str_regs = std::collections::HashSet::new();
    for raw in &code[start_pc + 1..] {
        let Ok(instr) = Instr::try_from_raw(*raw) else { break };
        match instr.opcode() {
            Opcode::Move if regs.contains(&instr.b()) => {
                regs.insert(instr.a());
            }
            Opcode::LoadString | Opcode::ConcatString | Opcode::ConcatN | Opcode::ToString => {
                str_regs.insert(instr.a());
            }
            // String `+` compiles to AddInt (runtime dispatch): the result is a
            // string iff an operand is.
            Opcode::AddInt => {
                if str_regs.contains(&instr.b()) || str_regs.contains(&instr.c()) {
                    str_regs.insert(instr.a());
                } else {
                    str_regs.remove(&instr.a());
                }
            }
            Opcode::LoadInt
            | Opcode::SubInt
            | Opcode::MulInt
            | Opcode::AddIntI
            | Opcode::MulIntI
            | Opcode::ModIntI
            | Opcode::ModInt => {
                str_regs.remove(&instr.a());
            }
            Opcode::SetFieldK if regs.contains(&instr.a()) => return false,
            Opcode::GetFieldK if regs.contains(&instr.b()) => return false,
            // Composite string-int keys are string-keyed by construction.
            Opcode::SetIndexStrI if regs.contains(&instr.a()) => return false,
            Opcode::GetIndexStrI if regs.contains(&instr.b()) => return false,
            Opcode::SetIndex if regs.contains(&instr.a()) => return !str_regs.contains(&instr.b()),
            Opcode::GetIndex | Opcode::GetList if regs.contains(&instr.b()) => {
                return !str_regs.contains(&instr.c());
            }
            _ => {}
        }
    }
    false
}

/// Whether an empty `[]` literal's first pushed element is a string (tracked
/// through `Move`s, like the map-key lookahead). A wrong guess only costs a
/// fallback: the typed push rejects the mismatch, never miscompiles.
fn empty_list_is_str_elem(code: &[u32], start_pc: usize, dst_reg: u8) -> bool {
    let mut regs = std::collections::HashSet::new();
    regs.insert(dst_reg);
    let mut str_regs = std::collections::HashSet::new();
    for raw in &code[start_pc + 1..] {
        let Ok(instr) = Instr::try_from_raw(*raw) else { break };
        match instr.opcode() {
            Opcode::Move if regs.contains(&instr.b()) => {
                regs.insert(instr.a());
            }
            Opcode::LoadString | Opcode::ConcatString | Opcode::ConcatN | Opcode::ToString => {
                str_regs.insert(instr.a());
            }
            Opcode::AddInt => {
                if str_regs.contains(&instr.b()) || str_regs.contains(&instr.c()) {
                    str_regs.insert(instr.a());
                } else {
                    str_regs.remove(&instr.a());
                }
            }
            Opcode::LoadInt
            | Opcode::LoadFloat
            | Opcode::SubInt
            | Opcode::MulInt
            | Opcode::AddIntI
            | Opcode::MulIntI
            | Opcode::ModIntI
            | Opcode::ModInt => {
                str_regs.remove(&instr.a());
            }
            Opcode::ListPush if regs.contains(&instr.a()) => {
                return str_regs.contains(&instr.b());
            }
            _ => {}
        }
    }
    false
}

/// Interns a string constant into the module globals, returning its [`GlobalId`]
/// index (deduplicating identical strings so repeated keys share one global).
fn intern_global(globals: &mut Vec<String>, s: &str) -> u32 {
    if let Some(i) = globals.iter().position(|g| g == s) {
        i as u32
    } else {
        globals.push(s.to_string());
        (globals.len() - 1) as u32
    }
}

/// Lowers a single function to a [`MirFunction`]. User (non-entry) functions use
/// the `(i64, ...) -> i64` ABI in this slice: params and return are `I64`, verified
/// via typed reads / a return-type check — a mismatch rejects (falls back) rather
/// than miscompiles.
#[allow(clippy::too_many_arguments)]
fn lower_function(
    func: &FunctionData,
    funcs: &[FunctionData],
    func_index: u32,
    entry: u32,
    is_entry: bool,
    globals: &mut Vec<String>,
    module_globals: &[String],
    sig: &mut SigInfer,
) -> Result<MirFunction, Unsupported> {
    if is_entry && func.capture_count != 0 {
        return Err(Unsupported::EntryHasCaptures(func.capture_count));
    }
    if is_entry && func.param_count != 0 {
        return Err(Unsupported::EntryHasParams(func.param_count));
    }
    let param_count = func.param_count as usize;
    let capture_count = func.capture_count as usize;

    let code_len = func.code.len();
    let instrs = func
        .code
        .iter()
        .enumerate()
        .map(|(pc, raw)| Instr::try_from_raw(*raw).map_err(|_| Unsupported::BadInstr { pc }))
        .collect::<Result<Vec<_>, _>>()?;

    // 1. Classify control-flow exits; a fused `TestXxx`+`Jmp` consumes the `Jmp`.
    let mut consumed = vec![false; code_len];
    let exits: Vec<Option<Exit>> = (0..code_len)
        .map(|pc| exit_of(pc, &instrs, code_len, &mut consumed, &func.performance))
        .collect::<Result<Vec<_>, _>>()?;

    // 2. Block leaders.
    let mut leaders = std::collections::BTreeSet::new();
    leaders.insert(0usize);
    let mut implicit_ret = false;
    for (pc, exit) in exits.iter().enumerate() {
        match exit {
            None => {}
            Some(Exit::Ret(_)) => {
                if pc + 1 < code_len {
                    leaders.insert(pc + 1);
                }
            }
            Some(Exit::Jump(t)) => {
                mark_target(*t, code_len, &mut leaders, &mut implicit_ret);
                if pc + 1 < code_len {
                    leaders.insert(pc + 1);
                }
            }
            Some(Exit::Cond { then_pc, else_pc, .. }) => {
                mark_target(*then_pc, code_len, &mut leaders, &mut implicit_ret);
                mark_target(*else_pc, code_len, &mut leaders, &mut implicit_ret);
                if pc + 1 < code_len {
                    leaders.insert(pc + 1);
                }
            }
            Some(Exit::FusedCmp { taken, fallthrough, .. })
            | Some(Exit::FusedCmp2 { taken, fallthrough, .. })
            | Some(Exit::ForLoop { taken, fallthrough, .. })
            | Some(Exit::FusedModZero { taken, fallthrough, .. })
            | Some(Exit::NilBranch { taken, fallthrough, .. }) => {
                mark_target(*taken, code_len, &mut leaders, &mut implicit_ret);
                mark_target(*fallthrough, code_len, &mut leaders, &mut implicit_ret);
            }
        }
    }

    // 3. Block ids (+ optional synthetic implicit-nil-return block).
    let leader_vec: Vec<usize> = leaders.iter().copied().collect();
    let pc_to_block: BTreeMap<usize, u32> = leader_vec.iter().enumerate().map(|(i, &pc)| (pc, i as u32)).collect();
    let implicit_ret_block = if implicit_ret {
        Some(leader_vec.len() as u32)
    } else {
        None
    };
    let block_of = |pc: usize| -> usize {
        if pc >= code_len {
            implicit_ret_block.expect("marked when a one-past-end target exists") as usize
        } else {
            *pc_to_block.range(..=pc).next_back().map(|(_, id)| id).unwrap() as usize
        }
    };

    // 4. Predecessors per block (edges over the CFG).
    let total_blocks = leader_vec.len() + usize::from(implicit_ret);
    let reg_count = func.register_count as usize;
    let mut preds: Vec<Vec<usize>> = vec![Vec::new(); total_blocks];
    let block_bounds: Vec<(usize, usize)> = leader_vec
        .iter()
        .enumerate()
        .map(|(bi, &start)| (start, leader_vec.get(bi + 1).copied().unwrap_or(code_len)))
        .collect();
    for (bi, &(start, end)) in block_bounds.iter().enumerate() {
        let (_, exit) = block_span(&exits, &consumed, start, end);
        for succ in exit_successors(exit, end) {
            preds[block_of(succ)].push(bi);
        }
    }

    // 5. Lower each block in leader order via Braun on-demand SSA construction.
    // One virtual cell slot per `LoadHeapConst UpvalCell` site (cell ids are
    // assigned in lowering order, so the site count bounds them).
    let cell_capacity = instrs
        .iter()
        .filter(|i| {
            i.opcode() == Opcode::LoadHeapConst
                && matches!(
                    func.consts.heap_values.get(i.bx() as usize),
                    Some(ConstHeapValueData::UpvalCell(_))
                )
        })
        .count();
    let mut ssa = Ssa::new(reg_count, cell_capacity, preds, total_blocks);
    // Function parameters occupy r0..r(param_count-1) at entry; each takes its
    // inferred type (the argument type observed at call sites, `I64` by default).
    // They seed the entry block's register file as its first SSA values.
    let identities: Vec<Option<LambdaIdentity>> =
        sig.lambda_params.get(func_index as usize).cloned().unwrap_or_default();
    let env_total: usize = identities.iter().flatten().map(|id| id.captures as usize).sum();
    let mut fn_params: Vec<(ValueId, Ty)> = Vec::with_capacity(param_count + env_total + capture_count);
    for r in 0..param_count {
        // An erased zero-capture lambda parameter has no runtime value: the
        // register holds the statically known function ref (indirect calls
        // devirtualize). Erased capturing identities bind below, after the
        // visible parameters, so signature order matches the call site.
        if let Some(id) = identities.get(r).copied().flatten() {
            if id.captures == 0 {
                ssa.builtin_regs.insert((0, r as u8), GlobalRef::Lambda(id.fidx));
            }
            continue;
        }
        let pty = sig.param_ty(func_index as usize, r);
        let pv = ssa.new_val();
        ssa.current_def[0][r] = Some((pv, pty));
        fn_params.push((pv, pty));
    }
    // An erased *capturing* closure argument: its environment (resolved at
    // the call site) arrives as hidden trailing parameters, one block per
    // erased parameter in parameter order. The register holds a Closure ref
    // whose captures alias those parameters by value.
    let mut env_offset = 0usize;
    for r in 0..param_count {
        let Some(id) = identities.get(r).copied().flatten() else {
            continue;
        };
        if id.captures == 0 {
            continue;
        }
        let mut caps = Vec::with_capacity(id.captures as usize);
        for _ in 0..id.captures {
            let ety = sig.param_ty(func_index as usize, param_count + env_offset);
            let ev = ssa.new_val();
            fn_params.push((ev, ety));
            caps.push(ClosureCapture::Value(ev, ety));
            env_offset += 1;
        }
        ssa.builtin_regs.insert((0, r as u8), GlobalRef::Closure(id.fidx, caps));
    }
    // A capturing lambda's own environment arrives after any erased-argument
    // env blocks (the closure's by-value snapshot, appended by the `Call`
    // lowering); it occupies no register — `LoadCapture k` reads it directly.
    let mut capture_params: Vec<(ValueId, Ty)> = Vec::with_capacity(capture_count);
    for k in 0..capture_count {
        let cty = sig.param_ty(func_index as usize, param_count + env_total + k);
        let cv = ssa.new_val();
        capture_params.push((cv, cty));
        fn_params.push((cv, cty));
    }
    let mut block_insts: Vec<Vec<Inst>> = vec![Vec::new(); total_blocks];
    let mut block_exit: Vec<Option<Exit>> = vec![None; total_blocks];
    let mut ret_ty: Option<Ty> = None;
    // Resolved terminator value reads (filled during each block's lowering).
    let mut ret_val: Vec<Option<ValueId>> = vec![None; total_blocks];
    let mut cond_val: Vec<Option<ValueId>> = vec![None; total_blocks];

    for (bi, &(start, end)) in block_bounds.iter().enumerate() {
        ssa.seal_ready()?;
        let (body_end, exit) = block_span(&exits, &consumed, start, end);
        if exit.is_none() {
            ssa.single_fallthrough_target[bi] = Some(end);
        }
        let mut insts = Vec::new();
        #[allow(clippy::needless_range_loop)] // `pc` is the semantic bytecode index
        for pc in start..body_end {
            lower_inst(
                &mut ssa,
                bi,
                &mut insts,
                func,
                funcs,
                entry,
                globals,
                module_globals,
                sig,
                &capture_params,
                &instrs[pc],
                pc,
            )?;
        }
        // Resolve the terminator's value reads while this block is current.
        match exit {
            Some(Exit::Ret(Some(reg))) => {
                // A return of a closure ref has no SSA value. When it is the
                // function's only return, the body is effect-free, and every
                // capture resolves to a parameter, record a summary — call
                // sites construct the closure from their argument values and
                // this body is never emitted. Everything else rejects below.
                if !is_entry && let Some(candidate) = ret_closure_candidate(&mut ssa, reg, bi, &fn_params, param_count)
                {
                    let single_ret =
                        !implicit_ret && exits.iter().flatten().filter(|e| matches!(e, Exit::Ret(_))).count() == 1;
                    if single_ret
                        && capture_count == 0
                        && identities.iter().all(Option::is_none)
                        && ret_closure_body_is_pure(&instrs)
                    {
                        record_ret_closure(sig, func_index as usize, candidate);
                    }
                    return Err(Unsupported::Opcode {
                        pc: start,
                        op: Opcode::Return1,
                    });
                }
                let (v, ty) = ssa.read(reg, bi, start)?;
                match ret_ty {
                    Some(prev) if prev != ty => return Err(Unsupported::ReturnTypeConflict),
                    _ => ret_ty = Some(ty),
                }
                // A `Nil` return value renders as `ret void`.
                ret_val[bi] = if ty == Ty::Nil { None } else { Some(v) };
            }
            Some(Exit::Cond { cond, .. }) => {
                let (v, ty) = ssa.read(cond, bi, start)?;
                if ty != Ty::Bool {
                    return Err(Unsupported::NonBoolCondition { pc: start });
                }
                cond_val[bi] = Some(v);
            }
            Some(Exit::FusedCmp { reg_a, rhs, op, .. }) => {
                // Dispatch on the tested register's type (int vs float compare).
                // A `Maybe` operand unwraps first (aborting when absent — the
                // VM's halt on comparing nil).
                let (lv, lty) = read_scalar(&mut ssa, &mut insts, reg_a, bi, start)?;
                let (float, lhs, rhs_val) = match lty {
                    Ty::I64 => {
                        let rhs_val = match rhs {
                            FusedRhs::Imm(n) => {
                                let c = ssa.new_val();
                                insts.push(Inst::Const {
                                    dst: c,
                                    value: Const::I64(n),
                                });
                                c
                            }
                            FusedRhs::Reg(r) => read_typed_scalar(&mut ssa, &mut insts, r, bi, Ty::I64, start)?,
                        };
                        (false, lv, rhs_val)
                    }
                    Ty::F64 => {
                        let rhs_val = match rhs {
                            FusedRhs::Imm(n) => {
                                let c = ssa.new_val();
                                insts.push(Inst::Const {
                                    dst: c,
                                    value: Const::F64(n as f64),
                                });
                                c
                            }
                            FusedRhs::Reg(r) => {
                                let (rv, rty) = ssa.read(r, bi, start)?;
                                coerce_to_f64(&mut ssa, &mut insts, rv, rty)
                            }
                        };
                        (true, lv, rhs_val)
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc: start }),
                };
                let cond = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: cond,
                    op,
                    float,
                    lhs,
                    rhs: rhs_val,
                });
                cond_val[bi] = Some(cond);
            }
            Some(Exit::ForLoop {
                index_reg,
                end_reg,
                step_reg,
                inclusive,
                positive_step,
                ..
            }) => {
                // next = index + step (wrapping, like the VM); the register is
                // updated *before* the branch so the back-edge phi carries it.
                let index = ssa.read_typed(index_reg, bi, Ty::I64, start)?;
                let end = ssa.read_typed(end_reg, bi, Ty::I64, start)?;
                let step = ssa.read_typed(step_reg, bi, Ty::I64, start)?;
                let next = ssa.new_val();
                insts.push(Inst::IntBin {
                    dst: next,
                    op: IntBinOp::Add,
                    lhs: index,
                    rhs: step,
                });
                ssa.write(index_reg, bi, (next, Ty::I64));
                let op = match (positive_step, inclusive) {
                    (true, true) => CmpOp::Le,
                    (true, false) => CmpOp::Lt,
                    (false, true) => CmpOp::Ge,
                    (false, false) => CmpOp::Gt,
                };
                let cond = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: cond,
                    op,
                    float: false,
                    lhs: next,
                    rhs: end,
                });
                cond_val[bi] = Some(cond);
            }
            Some(Exit::FusedCmp2 {
                reg_a,
                imm_a,
                reg_b,
                imm_b,
                ..
            }) => {
                let a = ssa.read_typed(reg_a, bi, Ty::I64, start)?;
                let b = ssa.read_typed(reg_b, bi, Ty::I64, start)?;
                let ka = ssa.new_val();
                insts.push(Inst::Const {
                    dst: ka,
                    value: Const::I64(imm_a),
                });
                let kb = ssa.new_val();
                insts.push(Inst::Const {
                    dst: kb,
                    value: Const::I64(imm_b),
                });
                let ca = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: ca,
                    op: CmpOp::Eq,
                    float: false,
                    lhs: a,
                    rhs: ka,
                });
                let cb = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: cb,
                    op: CmpOp::Eq,
                    float: false,
                    lhs: b,
                    rhs: kb,
                });
                let cond = ssa.new_val();
                insts.push(Inst::BoolAnd {
                    dst: cond,
                    lhs: ca,
                    rhs: cb,
                });
                cond_val[bi] = Some(cond);
            }
            Some(Exit::FusedModZero { reg_a, divisor, op, .. }) => {
                // `r_a % divisor <op> 0`: guarded modulo (aborts on a zero divisor,
                // matching the VM) then a compare against zero.
                let lhs = ssa.read_typed(reg_a, bi, Ty::I64, start)?;
                let d = ssa.new_val();
                insts.push(Inst::Const {
                    dst: d,
                    value: Const::I64(divisor),
                });
                let m = ssa.new_val();
                insts.push(Inst::IntBin {
                    dst: m,
                    op: IntBinOp::Mod,
                    lhs,
                    rhs: d,
                });
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::I64(0),
                });
                let cond = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: cond,
                    op,
                    float: false,
                    lhs: m,
                    rhs: zero,
                });
                cond_val[bi] = Some(cond);
            }
            Some(Exit::NilBranch {
                reg_a, jump_when_nil, ..
            }) => {
                // Resolve nil-ness by the operand's static type: a `Maybe` tests its
                // present bit; any other scalar is provably non-nil (and `Ty::Nil` is
                // provably nil), so the branch folds to a constant. The `cond` is true
                // exactly when the `taken` edge should be followed.
                let (v, ty) = ssa.read(reg_a, bi, start)?;
                let cond = match ty {
                    Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                        let present = ssa.new_val();
                        insts.push(Inst::MaybePresent {
                            dst: present,
                            src: v,
                            maybe_ty: ty,
                        });
                        if jump_when_nil {
                            // taken when nil = when NOT present.
                            let c = ssa.new_val();
                            insts.push(Inst::Not { dst: c, src: present });
                            c
                        } else {
                            // taken when not-nil = present.
                            present
                        }
                    }
                    Ty::Nil => {
                        let c = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: c,
                            value: Const::Bool(jump_when_nil),
                        });
                        c
                    }
                    _ => {
                        let c = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: c,
                            value: Const::Bool(!jump_when_nil),
                        });
                        c
                    }
                };
                cond_val[bi] = Some(cond);
            }
            _ => {}
        }
        block_insts[bi] = insts;
        block_exit[bi] = exit;
        ssa.mark_filled(bi);
        ssa.seal_ready()?;
    }
    if let Some(id) = implicit_ret_block {
        ssa.mark_filled(id as usize);
    }
    ssa.seal_ready()?;

    // 6. Build MIR blocks: block params come from the constructed phis; branch args
    //    come from each successor phi's operand contributed by this block.
    let block_id = |pc: usize| -> u32 {
        if pc >= code_len {
            implicit_ret_block.expect("implicit ret block present")
        } else {
            *pc_to_block.range(..=pc).next_back().map(|(_, id)| id).unwrap()
        }
    };
    let mut mir_blocks: Vec<Block> = Vec::with_capacity(total_blocks);
    for bi in 0..leader_vec.len() {
        let params: Vec<(ValueId, Ty)> = ssa.phis[bi].iter().map(|p| (p.param, p.ty)).collect();
        let exit = block_exit[bi];
        // Phi-edge conversions land after the block's own instructions,
        // before the terminator.
        let edge_tail = std::mem::take(&mut ssa.edge_insts[bi]);
        let term = build_term(bi, exit, &ssa, &block_id, ret_val[bi], cond_val[bi]);
        let mut insts = std::mem::take(&mut block_insts[bi]);
        insts.extend(edge_tail);
        mir_blocks.push(Block {
            id: BlockId(bi as u32),
            params,
            insts,
            term,
        });
    }
    if let Some(id) = implicit_ret_block {
        let params: Vec<(ValueId, Ty)> = ssa.phis[id as usize].iter().map(|p| (p.param, p.ty)).collect();
        mir_blocks.push(Block {
            id: BlockId(id),
            params,
            insts: Vec::new(),
            term: Term::Ret(None),
        });
    }

    let ret = ret_ty.unwrap_or(Ty::Nil);
    // User (non-entry) functions return scalars, `Str`/handle pointers
    // (arena-owned until exit), or nothing (`Nil` renders as `void`).
    // Returning a `Maybe` carrier across the direct-call boundary isn't
    // modelled — reject (fall back).
    if !is_entry && matches!(ret, Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
        return Err(Unsupported::ReturnTypeConflict);
    }
    // The entry can return scalars (printed), but not a container handle (printing
    // a list is not modelled yet) — reject so it falls back rather than print wrong.
    if is_entry
        && matches!(
            ret,
            Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::MapStrI64 | Ty::MapI64I64 | Ty::MapStrF64 | Ty::MapI64F64
        )
    {
        return Err(Unsupported::ReturnTypeConflict);
    }
    Ok(MirFunction {
        id: FuncId(func_index),
        params: fn_params,
        entry: BlockId(0),
        ret,
        blocks: mir_blocks,
    })
}

/// Branch args this block passes to `target`: one per target phi, taken from the
/// operand that phi recorded for the `from` predecessor.
fn args_to(ssa: &Ssa, from: usize, target: usize) -> Vec<ValueId> {
    ssa.phis[target]
        .iter()
        .map(|phi| {
            phi.operands
                .iter()
                .find(|(pred, _)| *pred == from)
                .map(|(_, v)| *v)
                .expect("sealed phi has an operand for every predecessor")
        })
        .collect()
}

fn build_term(
    bi: usize,
    exit: Option<Exit>,
    ssa: &Ssa,
    block_id: &impl Fn(usize) -> u32,
    ret_val: Option<ValueId>,
    cond_val: Option<ValueId>,
) -> Term {
    let br = |target_pc: usize| -> Term {
        let t = block_id(target_pc);
        Term::Br {
            target: BlockId(t),
            args: args_to(ssa, bi, t as usize),
        }
    };
    match exit {
        None => {
            // Fall through to the next block (its leader is `end`, recovered here as
            // the sole successor recorded in the CFG).
            let succ = ssa.single_fallthrough_target[bi].expect("fallthrough target");
            let t = block_id(succ);
            Term::Br {
                target: BlockId(t),
                args: args_to(ssa, bi, t as usize),
            }
        }
        Some(Exit::Ret(None)) => Term::Ret(None),
        // `ret_val` is `None` for a resolved `Nil` return value (`ret void`).
        Some(Exit::Ret(Some(_))) => Term::Ret(ret_val),
        Some(Exit::Jump(pc)) => br(pc),
        Some(Exit::Cond { then_pc, else_pc, .. }) => {
            let cond = cond_val.expect("cond resolved");
            let t = block_id(then_pc);
            let e = block_id(else_pc);
            Term::CondBr {
                cond,
                then_blk: BlockId(t),
                then_args: args_to(ssa, bi, t as usize),
                else_blk: BlockId(e),
                else_args: args_to(ssa, bi, e as usize),
            }
        }
        Some(Exit::FusedCmp {
            jump_when,
            taken,
            fallthrough,
            ..
        }) => {
            let cond = cond_val.expect("fused cond resolved");
            let taken_b = block_id(taken);
            let fall_b = block_id(fallthrough);
            let (then_b, else_b) = if jump_when {
                (taken_b, fall_b)
            } else {
                (fall_b, taken_b)
            };
            Term::CondBr {
                cond,
                then_blk: BlockId(then_b),
                then_args: args_to(ssa, bi, then_b as usize),
                else_blk: BlockId(else_b),
                else_args: args_to(ssa, bi, else_b as usize),
            }
        }
        // The range condition holding means "loop back" (`taken`).
        Some(Exit::ForLoop { taken, fallthrough, .. }) => {
            let cond = cond_val.expect("for-loop cond resolved");
            let then_b = block_id(taken);
            let else_b = block_id(fallthrough);
            Term::CondBr {
                cond,
                then_blk: BlockId(then_b),
                then_args: args_to(ssa, bi, then_b as usize),
                else_blk: BlockId(else_b),
                else_args: args_to(ssa, bi, else_b as usize),
            }
        }
        // The conjunction holding means "fall through"; anything else takes the
        // branch (the VM's false-branch application for `TestEqIntI2`).
        Some(Exit::FusedCmp2 { taken, fallthrough, .. }) => {
            let cond = cond_val.expect("fused-cmp2 cond resolved");
            let then_b = block_id(fallthrough);
            let else_b = block_id(taken);
            Term::CondBr {
                cond,
                then_blk: BlockId(then_b),
                then_args: args_to(ssa, bi, then_b as usize),
                else_blk: BlockId(else_b),
                else_args: args_to(ssa, bi, else_b as usize),
            }
        }
        // The compare already encodes the polarity (`op`), so `taken` is always the
        // `then` branch.
        Some(Exit::FusedModZero { taken, fallthrough, .. }) => {
            let cond = cond_val.expect("fused-mod cond resolved");
            let then_b = block_id(taken);
            let else_b = block_id(fallthrough);
            Term::CondBr {
                cond,
                then_blk: BlockId(then_b),
                then_args: args_to(ssa, bi, then_b as usize),
                else_blk: BlockId(else_b),
                else_args: args_to(ssa, bi, else_b as usize),
            }
        }
        // `cond` was resolved so that it is true exactly on the `taken` edge.
        Some(Exit::NilBranch { taken, fallthrough, .. }) => {
            let cond = cond_val.expect("nil-branch cond resolved");
            let then_b = block_id(taken);
            let else_b = block_id(fallthrough);
            Term::CondBr {
                cond,
                then_blk: BlockId(then_b),
                then_args: args_to(ssa, bi, then_b as usize),
                else_blk: BlockId(else_b),
                else_args: args_to(ssa, bi, else_b as usize),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Braun on-demand SSA construction (adapted to MIR block params + branch args).
// ---------------------------------------------------------------------------

/// A phi = a block parameter for one register, plus its per-predecessor operands.
struct Phi {
    param: ValueId,
    reg: usize,
    ty: Ty,
    operands: Vec<(usize, ValueId)>,
}

struct Ssa {
    reg_count: usize,
    /// Register slots plus the virtual cell slots appended after them
    /// (`reg_count + cid` addresses cell `cid`'s content).
    slot_count: usize,
    preds: Vec<Vec<usize>>,
    current_def: Vec<Vec<Option<Reg>>>,
    sealed: Vec<bool>,
    filled: Vec<bool>,
    phis: Vec<Vec<Phi>>,
    incomplete: Vec<Vec<usize>>,
    /// For fallthrough (`None` exit) blocks, the sole successor block's leader pc.
    single_fallthrough_target: Vec<Option<usize>>,
    next_val: u32,
    /// Compile-time-known values, for provably-in-bounds constant list indexing:
    /// SSA value → its constant `i64` (recorded for direct `LoadInt`s).
    const_int: std::collections::HashMap<ValueId, i64>,
    /// SSA value of a const-materialized list handle → its known element count.
    list_len: std::collections::HashMap<ValueId, i64>,
    /// Element count at materialization (never bumped by pushes): a sound
    /// *lower bound* on the runtime length — the subset has no removal ops,
    /// while `list_len`'s static push increments can overshoot (a push in an
    /// untaken branch). Used to prove both operands of a cross-typed list
    /// comparison non-empty.
    list_base_len: std::collections::HashMap<ValueId, i64>,
    /// SSA value → its compile-time string content (`LoadString` /
    /// `LoadHeapConst` long strings), used to expand `println` format strings
    /// at lower time.
    const_strs: std::collections::HashMap<ValueId, String>,
    /// `(block, register)` → global reference (runtime builtin, module object,
    /// or resolved module function) loaded there by `GetGlobal`/`GetIndex` and
    /// propagated by `Move`. Block-local by construction; any write to the
    /// register clears it.
    builtin_regs: std::collections::HashMap<(usize, u8), GlobalRef>,
    /// Fresh ids for upvalue cells created by `LoadHeapConst`; each cell's
    /// content lives in virtual slot `reg_count + cid`, participating in the
    /// same Braun construction as registers (cross-block cell state gets
    /// phis). Iteration isolation needs no extra guard: the only path to a
    /// cell read is a `Cell`/`Closure` ref in `builtin_regs`, and ref
    /// propagation dies at loop headers whose entry edge lacks the ref, while
    /// the creation site re-initializes the slot each iteration.
    next_cell: u32,
    /// Per-block trailing instructions added for phi-edge type conversions
    /// (`Maybe` ↔ scalar merges); appended after the block's own instructions
    /// when the MIR blocks are assembled.
    edge_insts: Vec<Vec<Inst>>,
}

impl Ssa {
    fn new(reg_count: usize, cell_capacity: usize, preds: Vec<Vec<usize>>, total_blocks: usize) -> Self {
        let slot_count = reg_count + cell_capacity;
        Self {
            reg_count,
            slot_count,
            preds,
            current_def: vec![vec![None; slot_count]; total_blocks],
            sealed: vec![false; total_blocks],
            filled: vec![false; total_blocks],
            phis: (0..total_blocks).map(|_| Vec::new()).collect(),
            incomplete: (0..total_blocks).map(|_| Vec::new()).collect(),
            single_fallthrough_target: vec![None; total_blocks],
            next_val: 0,
            const_int: std::collections::HashMap::new(),
            list_len: std::collections::HashMap::new(),
            list_base_len: std::collections::HashMap::new(),
            const_strs: std::collections::HashMap::new(),
            builtin_regs: std::collections::HashMap::new(),
            next_cell: 0,
            edge_insts: vec![Vec::new(); total_blocks],
        }
    }

    fn new_val(&mut self) -> ValueId {
        let v = ValueId(self.next_val);
        self.next_val += 1;
        v
    }

    fn write(&mut self, reg: u8, block: usize, value: Reg) {
        if (reg as usize) < self.reg_count {
            self.write_slot(reg as usize, block, value);
        }
    }

    fn write_slot(&mut self, slot: usize, block: usize, value: Reg) {
        if slot < self.slot_count {
            self.current_def[block][slot] = Some(value);
            if slot < self.reg_count {
                self.builtin_regs.remove(&(block, slot as u8));
            }
        }
    }

    fn read(&mut self, reg: u8, block: usize, pc: usize) -> Result<Reg, Unsupported> {
        self.read_slot(reg as usize, block, pc)
    }

    fn read_slot(&mut self, slot: usize, block: usize, pc: usize) -> Result<Reg, Unsupported> {
        if let Some(v) = self.current_def[block][slot] {
            return Ok(v);
        }
        self.read_recursive(slot, block, pc)
    }

    /// The virtual slot holding cell `cid`'s content.
    fn cell_slot(&self, cid: u32) -> usize {
        self.reg_count + cid as usize
    }

    fn read_typed(&mut self, reg: u8, block: usize, want: Ty, pc: usize) -> Result<ValueId, Unsupported> {
        let (v, ty) = self.read(reg, block, pc)?;
        if ty == want {
            Ok(v)
        } else {
            Err(Unsupported::TypeMismatch { pc })
        }
    }

    /// Resolves the compile-time string content of `reg` at `block`, if every
    /// acyclic reaching definition is the same constant string. Read-only:
    /// unlike `read`, this never creates phis, so it can look through unsealed
    /// loop headers (a cycle path contributes no new definition). Recovers
    /// `println` format strings the compiler's loop-literal cache hoisted out
    /// of the loop body (where the plain `const_strs` value lookup only sees
    /// the loop-header phi).
    /// Resolves the global ref a register holds at `block`, backtracking
    /// through predecessors when the loading block differs from the using
    /// block (e.g. `assert(a || b)` — the short-circuit's merge block calls a
    /// builtin loaded before the branch). All paths must agree on the same
    /// ref, and a block with an SSA definition for the register shadows it.
    fn builtin_ref_at(&self, reg: u8, block: usize) -> Option<GlobalRef> {
        let mut visited = std::collections::HashSet::new();
        let mut found: Option<GlobalRef> = None;
        if self.collect_builtin_ref(reg, block, &mut visited, &mut found) {
            found
        } else {
            None
        }
    }

    fn collect_builtin_ref(
        &self,
        reg: u8,
        block: usize,
        visited: &mut std::collections::HashSet<(usize, u8)>,
        found: &mut Option<GlobalRef>,
    ) -> bool {
        if !visited.insert((block, reg)) {
            return true;
        }
        if let Some(global_ref) = self.builtin_regs.get(&(block, reg)) {
            return match found {
                Some(prev) => prev == global_ref,
                None => {
                    *found = Some(global_ref.clone());
                    true
                }
            };
        }
        // An SSA definition in this block shadows any inherited ref.
        if self.current_def[block][reg as usize].is_some() {
            return false;
        }
        if self.preds[block].is_empty() {
            return false;
        }
        self.preds[block]
            .iter()
            .all(|&pred| self.collect_builtin_ref(reg, pred, visited, found))
    }

    fn reg_const_str(&self, reg: u8, block: usize) -> Option<String> {
        let mut visited = std::collections::HashSet::new();
        let mut found: Option<String> = None;
        if self.collect_reg_const_str(reg as usize, block, &mut visited, &mut found) {
            found
        } else {
            None
        }
    }

    fn collect_reg_const_str(
        &self,
        reg: usize,
        block: usize,
        visited: &mut std::collections::HashSet<(usize, usize)>,
        found: &mut Option<String>,
    ) -> bool {
        if !visited.insert((block, reg)) {
            return true;
        }
        if let Some((v, ty)) = self.current_def[block][reg] {
            if ty != Ty::Str {
                return false;
            }
            if let Some(s) = self.const_strs.get(&v) {
                return match found {
                    Some(prev) => prev == s,
                    None => {
                        *found = Some(s.clone());
                        true
                    }
                };
            }
            // A phi param's operands are exactly its register's reaching
            // definitions at the phi's own block — redirect the walk there
            // (the phi may be for a *different* register than the one that
            // carried the value here, e.g. through a `Move`). Any other
            // non-constant definition makes the value dynamic.
            for (phi_block, phis) in self.phis.iter().enumerate() {
                if let Some(phi) = phis.iter().find(|phi| phi.param == v) {
                    let phi_reg = phi.reg;
                    for p in self.preds[phi_block].clone() {
                        if !self.collect_reg_const_str(phi_reg, p, visited, found) {
                            return false;
                        }
                    }
                    return true;
                }
            }
            return false;
        }
        for &p in &self.preds[block] {
            if !self.collect_reg_const_str(reg, p, visited, found) {
                return false;
            }
        }
        true
    }

    /// The type of `reg` as seen from an already-filled predecessor (loop-invariant
    /// for the register classes we lower), used to type a freshly created phi.
    fn phi_ty(&mut self, slot: usize, block: usize, pc: usize) -> Result<Ty, Unsupported> {
        let preds = self.preds[block].clone();
        for p in preds {
            if self.filled[p] {
                return Ok(self.read_slot(slot, p, pc)?.1);
            }
        }
        Err(Unsupported::UndefinedOperand { pc, reg: slot })
    }

    fn read_recursive(&mut self, slot: usize, block: usize, pc: usize) -> Result<Reg, Unsupported> {
        let value: Reg = if !self.sealed[block] {
            let ty = self.phi_ty(slot, block, pc)?;
            let param = self.new_val();
            let idx = self.phis[block].len();
            self.phis[block].push(Phi {
                param,
                reg: slot,
                ty,
                operands: Vec::new(),
            });
            self.incomplete[block].push(idx);
            (param, ty)
        } else if self.preds[block].len() == 1 {
            let p = self.preds[block][0];
            self.read_slot(slot, p, pc)?
        } else if self.preds[block].is_empty() {
            return Err(Unsupported::UndefinedOperand { pc, reg: slot });
        } else {
            let ty = self.phi_ty(slot, block, pc)?;
            let param = self.new_val();
            let idx = self.phis[block].len();
            self.phis[block].push(Phi {
                param,
                reg: slot,
                ty,
                operands: Vec::new(),
            });
            // Break cycles before reading operands.
            self.current_def[block][slot] = Some((param, ty));
            self.add_phi_operands(block, idx, pc)?;
            (param, ty)
        };
        self.current_def[block][slot] = Some(value);
        Ok(value)
    }

    fn add_phi_operands(&mut self, block: usize, phi_idx: usize, pc: usize) -> Result<(), Unsupported> {
        let slot = self.phis[block][phi_idx].reg;
        let phi_ty = self.phis[block][phi_idx].ty;
        let preds = self.preds[block].clone();
        for p in preds {
            let (v, ty) = self.read_slot(slot, p, pc)?;
            // Every incoming edge must agree on the type (the phi was typed from
            // one filled predecessor). A `Maybe` merging with its scalar (the
            // `let v = m[k]; if v == nil { v = default; }` shape) converts on
            // the incoming edge: extracting the raw value never observes the
            // absent case (the phi takes the other edge there), and wrapping a
            // scalar marks it present. Anything else is a heterogeneous value
            // our typed MIR cannot represent — reject (fall back).
            let v = if ty == phi_ty {
                v
            } else if let Some(converted) = self.convert_phi_edge(v, ty, phi_ty, p) {
                converted
            } else {
                return Err(Unsupported::TypeMismatch { pc });
            };
            self.phis[block][phi_idx].operands.push((p, v));
        }
        Ok(())
    }

    fn convert_phi_edge(&mut self, v: ValueId, from: Ty, to: Ty, pred: usize) -> Option<ValueId> {
        let dst = match (from, to) {
            (Ty::MaybeI64, Ty::I64) | (Ty::MaybeF64, Ty::F64) | (Ty::MaybeStr, Ty::Str) => {
                let dst = self.new_val();
                self.edge_insts[pred].push(Inst::MaybeValue {
                    dst,
                    src: v,
                    maybe_ty: from,
                });
                dst
            }
            (Ty::I64, Ty::MaybeI64) | (Ty::F64, Ty::MaybeF64) | (Ty::Str, Ty::MaybeStr) => {
                let dst = self.new_val();
                self.edge_insts[pred].push(Inst::MaybeWrap {
                    dst,
                    src: v,
                    maybe_ty: to,
                });
                dst
            }
            _ => return None,
        };
        Some(dst)
    }

    fn mark_filled(&mut self, block: usize) {
        self.filled[block] = true;
    }

    /// Seals every unsealed block whose predecessors are all filled (a fixpoint,
    /// so sealing a loop header after its back-edge predecessor fills takes effect).
    fn seal_ready(&mut self) -> Result<(), Unsupported> {
        loop {
            let mut progressed = false;
            for b in 0..self.sealed.len() {
                if !self.sealed[b] && self.preds[b].iter().all(|&p| self.filled[p]) {
                    self.seal_block(b)?;
                    progressed = true;
                }
            }
            if !progressed {
                return Ok(());
            }
        }
    }

    fn seal_block(&mut self, block: usize) -> Result<(), Unsupported> {
        let incs = std::mem::take(&mut self.incomplete[block]);
        for idx in incs {
            self.add_phi_operands(block, idx, 0)?;
        }
        self.sealed[block] = true;
        Ok(())
    }
}

fn mark_target(t: usize, code_len: usize, leaders: &mut std::collections::BTreeSet<usize>, implicit_ret: &mut bool) {
    if t >= code_len {
        *implicit_ret = true;
    } else {
        leaders.insert(t);
    }
}

/// `(body_end, exit)` for block `[start, end)`. A fused compare-and-branch occupies
/// the last two slots (`TestXxx` at `end-2`, consumed `Jmp` at `end-1`).
fn block_span(exits: &[Option<Exit>], consumed: &[bool], start: usize, end: usize) -> (usize, Option<Exit>) {
    if end >= start + 2 && consumed[end - 1] {
        return (end - 2, exits[end - 2]);
    }
    if end > start
        && let Some(exit) = exits[end - 1]
    {
        return (end - 1, Some(exit));
    }
    (end, None)
}

fn exit_successors(exit: Option<Exit>, fallthrough: usize) -> Vec<usize> {
    match exit {
        None => vec![fallthrough],
        Some(Exit::Ret(_)) => vec![],
        Some(Exit::Jump(t)) => vec![t],
        Some(Exit::Cond { then_pc, else_pc, .. }) => vec![then_pc, else_pc],
        Some(Exit::FusedCmp { taken, fallthrough, .. })
        | Some(Exit::FusedCmp2 { taken, fallthrough, .. })
        | Some(Exit::ForLoop { taken, fallthrough, .. })
        | Some(Exit::FusedModZero { taken, fallthrough, .. })
        | Some(Exit::NilBranch { taken, fallthrough, .. }) => {
            vec![taken, fallthrough]
        }
    }
}

fn exit_of(
    pc: usize,
    instrs: &[Instr],
    code_len: usize,
    consumed: &mut [bool],
    facts: &lk_core::vm::analysis::PerformanceFacts,
) -> Result<Option<Exit>, Unsupported> {
    if consumed[pc] {
        return Ok(None);
    }
    let instr = instrs[pc];
    if instr.opcode().is_compare_test() {
        let jmp = instrs.get(pc + 1).copied().ok_or(Unsupported::BadTarget { pc })?;
        if jmp.opcode() != Opcode::Jmp {
            return Err(Unsupported::Opcode { pc, op: instr.opcode() });
        }
        if instr.opcode() == Opcode::TestEqIntI2 {
            // `r_a == (c >> 4) && r_b == (c & 0xf)`: true falls through, false
            // takes the trailing `Jmp` (the VM's false-branch application).
            let packed = instr.c();
            let taken = rel(pc + 1, jmp.sj_arg(), code_len).ok_or(Unsupported::BadTarget { pc })?;
            consumed[pc + 1] = true;
            return Ok(Some(Exit::FusedCmp2 {
                reg_a: instr.a(),
                imm_a: i64::from(packed >> 4),
                reg_b: instr.b(),
                imm_b: i64::from(packed & 0x0f),
                taken,
                fallthrough: pc + 2,
            }));
        }
        let op = test_cmp_op(instr.opcode()).ok_or(Unsupported::Opcode { pc, op: instr.opcode() })?;
        let immediate = instr.opcode().is_int_immediate_compare_test();
        let rhs = if immediate {
            FusedRhs::Imm(instr.sc() as i64)
        } else {
            FusedRhs::Reg(instr.b())
        };
        let jump_when = if immediate { instr.b() != 0 } else { instr.c() != 0 };
        let taken = rel(pc + 1, jmp.sj_arg(), code_len).ok_or(Unsupported::BadTarget { pc })?;
        consumed[pc + 1] = true;
        return Ok(Some(Exit::FusedCmp {
            reg_a: instr.a(),
            rhs,
            op,
            jump_when,
            taken,
            fallthrough: pc + 2,
        }));
    }
    match instr.opcode() {
        Opcode::Return | Opcode::Return1 => Ok(Some(Exit::Ret(Some(instr.a())))),
        Opcode::Return0 => Ok(Some(Exit::Ret(None))),
        Opcode::Jmp => {
            let t = rel(pc, instr.sj_arg(), code_len).ok_or(Unsupported::BadTarget { pc })?;
            Ok(Some(Exit::Jump(t)))
        }
        // Fused compare-and-branch against an immediate (a single instruction, no
        // trailing `Jmp`): `if (r_a <op> imm) goto target else fall through`. The VM
        // requires an `Int` operand; the immediate is an unsigned byte. These are
        // what `if (i == k)` / `!=` inside loops lower to (enabling break/continue/
        // early-return/else-if shapes).
        Opcode::ForLoopI => {
            let Some(fact) = facts.for_loop(pc) else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            let taken = rel(pc, fact.jump_offset, code_len).ok_or(Unsupported::BadTarget { pc })?;
            Ok(Some(Exit::ForLoop {
                index_reg: instr.a(),
                end_reg: instr.b(),
                step_reg: instr.c(),
                inclusive: fact.inclusive,
                positive_step: fact.positive_step,
                taken,
                fallthrough: pc + 1,
            }))
        }
        Opcode::BrEqIntI4 | Opcode::BrNeIntI4 => {
            let taken = rel(pc, instr.branch_i4_offset() as i32, code_len).ok_or(Unsupported::BadTarget { pc })?;
            let op = if instr.opcode() == Opcode::BrEqIntI4 {
                CmpOp::Eq
            } else {
                CmpOp::Ne
            };
            Ok(Some(Exit::FusedCmp {
                reg_a: instr.a(),
                rhs: FusedRhs::Imm(i64::from(instr.branch_i4_immediate())),
                op,
                jump_when: true,
                taken,
                fallthrough: pc + 1,
            }))
        }
        // `if (r_a == 0)` / `!= 0` fused branch (immediate zero).
        Opcode::BrEqZeroInt | Opcode::BrNeZeroInt => {
            let taken = rel(pc, instr.sbx() as i32, code_len).ok_or(Unsupported::BadTarget { pc })?;
            let op = if instr.opcode() == Opcode::BrEqZeroInt {
                CmpOp::Eq
            } else {
                CmpOp::Ne
            };
            Ok(Some(Exit::FusedCmp {
                reg_a: instr.a(),
                rhs: FusedRhs::Imm(0),
                op,
                jump_when: true,
                taken,
                fallthrough: pc + 1,
            }))
        }
        // `if (x == nil)` / `!= nil` fused branch (offset in `sbx`).
        Opcode::BrNil | Opcode::BrNotNil => {
            let taken = rel(pc, instr.sbx() as i32, code_len).ok_or(Unsupported::BadTarget { pc })?;
            Ok(Some(Exit::NilBranch {
                reg_a: instr.a(),
                jump_when_nil: instr.opcode() == Opcode::BrNil,
                taken,
                fallthrough: pc + 1,
            }))
        }
        // `if (r_a % k == 0)` / `!= 0` fused divisibility branch.
        Opcode::BrModEqZeroIntI4 | Opcode::BrModNeZeroIntI4 => {
            let taken = rel(pc, instr.branch_i4_offset() as i32, code_len).ok_or(Unsupported::BadTarget { pc })?;
            let op = if instr.opcode() == Opcode::BrModEqZeroIntI4 {
                CmpOp::Eq
            } else {
                CmpOp::Ne
            };
            Ok(Some(Exit::FusedModZero {
                reg_a: instr.a(),
                divisor: i64::from(instr.branch_i4_immediate()),
                op,
                taken,
                fallthrough: pc + 1,
            }))
        }
        Opcode::Test | Opcode::BrFalse | Opcode::BrTrue => {
            let relative = match instr.opcode() {
                Opcode::Test => rel(pc, instr.c() as i8 as i32, code_len),
                _ => rel(pc, instr.sbx() as i32, code_len),
            }
            .ok_or(Unsupported::BadTarget { pc })?;
            let fallthrough = pc + 1;
            let then_pc =
                if matches!(instr.opcode(), Opcode::Test if instr.b() == 0) || instr.opcode() == Opcode::BrTrue {
                    relative
                } else {
                    fallthrough
                };
            let else_pc =
                if matches!(instr.opcode(), Opcode::Test if instr.b() != 0) || instr.opcode() == Opcode::BrFalse {
                    relative
                } else {
                    fallthrough
                };
            Ok(Some(Exit::Cond {
                cond: instr.a(),
                then_pc,
                else_pc,
            }))
        }
        _ => Ok(None),
    }
}

fn rel(pc: usize, offset: i32, code_len: usize) -> Option<usize> {
    let target = pc as i64 + 1 + offset as i64;
    if target < 0 || target as usize > code_len {
        None
    } else {
        Some(target as usize)
    }
}

/// A `Ret` of a register holding a closure ref whose captures all resolve
/// (in the returning block) to the function's own parameter values.
fn ret_closure_candidate(
    ssa: &mut Ssa,
    reg: u8,
    block: usize,
    fn_params: &[(ValueId, Ty)],
    param_count: usize,
) -> Option<(u32, Vec<RetCaptureSrc>)> {
    let (fidx, caps) = match ssa.builtin_ref_at(reg, block)? {
        GlobalRef::Lambda(fidx) => (fidx, Vec::new()),
        GlobalRef::Closure(fidx, caps) => (fidx, caps),
        _ => return None,
    };
    let mut srcs = Vec::with_capacity(caps.len());
    for cap in &caps {
        let (v, _) = match cap {
            ClosureCapture::Cell(cid) => {
                let slot = ssa.cell_slot(*cid);
                ssa.read_slot(slot, block, 0).ok()?
            }
            ClosureCapture::Value(v, ty) => (*v, *ty),
        };
        let k = fn_params
            .get(..param_count.min(fn_params.len()))?
            .iter()
            .position(|&(pv, _)| pv == v)?;
        srcs.push(RetCaptureSrc::Param(k));
    }
    Some((fidx, srcs))
}

/// Effect-free body whitelist for [`SigInfer::ret_closures`]: constant loads,
/// register/cell moves, and the closure construction itself. Anything that can
/// abort, write observable state, or call out disqualifies the summary —
/// callers skip the call entirely, so a lost effect would diverge from the VM.
fn ret_closure_body_is_pure(instrs: &[Instr]) -> bool {
    instrs.iter().all(|instr| {
        matches!(
            instr.opcode(),
            Opcode::LoadNil
                | Opcode::LoadBool
                | Opcode::LoadInt
                | Opcode::LoadFloat
                | Opcode::LoadHeapConst
                | Opcode::StoreCellVal
                | Opcode::LoadCellVal
                | Opcode::Move
                | Opcode::Move2
                | Opcode::MakeClosure
                | Opcode::Return1
        )
    })
}

/// Records a closure-return summary; disagreeing returns poison the function
/// (no summary, so it rejects on lowering instead of miscompiling).
fn record_ret_closure(sig: &mut SigInfer, fi: usize, candidate: (u32, Vec<RetCaptureSrc>)) {
    if sig.ret_closure_poisoned.get(fi).copied().unwrap_or(true) {
        return;
    }
    let Some(slot) = sig.ret_closures.get_mut(fi) else {
        return;
    };
    match slot {
        None => *slot = Some(candidate),
        Some(prev) if *prev == candidate => {}
        Some(_) => {
            *slot = None;
            sig.ret_closure_poisoned[fi] = true;
        }
    }
}

/// Lowers a call to user function `callee_idx` with the register-window layout
/// shared by `CallDirect` and indirect `Call` (callee/result at `dst_reg`,
/// args at `[dst_reg+1, dst_reg+1+argc)`): reads the typed arguments, refines
/// the callee's per-callsite-monomorphized signature, and writes the typed
/// result. A capturing closure passes its environment snapshot as hidden
/// trailing arguments (`captures`); their count must match the callee's
/// `capture_count` (a `CallDirect` to a capturing lambda has no environment
/// and rejects).
#[allow(clippy::too_many_arguments)]
fn lower_user_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    callee_idx: usize,
    dst_reg: u8,
    argc: usize,
    captures: &[(ValueId, Ty)],
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    if callee_idx >= funcs.len() || callee_idx == entry as usize {
        return Err(Unsupported::Opcode {
            pc,
            op: Opcode::CallDirect,
        });
    }
    if argc != funcs[callee_idx].param_count as usize {
        return Err(Unsupported::Opcode {
            pc,
            op: Opcode::CallDirect,
        });
    }
    if captures.len() != funcs[callee_idx].capture_count as usize {
        return Err(Unsupported::Opcode {
            pc,
            op: Opcode::CallDirect,
        });
    }
    // Zero-capture lambda arguments are erased from the native signature:
    // collect the call site's lambda identity vector; a non-empty vector
    // retargets the call to a per-identity *clone* of the callee (created on
    // demand, byte-identical body, `lambda_params` pre-filled so its
    // parameters seed static refs instead of binding values).
    // Identity resolution backtracks across blocks like the hidden-env
    // lookup below (an argument register may inherit its lambda/closure ref
    // from a predecessor), so both paths agree on what the register holds.
    let identity: Vec<Option<LambdaIdentity>> = (0..argc)
        .map(|i| {
            let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
            match ssa.builtin_ref_at(arg_reg, block) {
                Some(GlobalRef::Lambda(fidx)) => Some(LambdaIdentity { fidx, captures: 0 }),
                Some(GlobalRef::Closure(fidx, caps)) => Some(LambdaIdentity {
                    fidx,
                    captures: caps.len() as u16,
                }),
                _ => None,
            }
        })
        .collect();
    let callee_idx = if identity.iter().any(Option::is_some) {
        // The clone carries `lambda_params`; the original body would treat the
        // parameter as a plain value and reject. A function called both ways
        // is polymorphic over functions vs values — outside the subset.
        if let Some(flag) = sig.specialized.get_mut(callee_idx) {
            *flag = true;
        }
        if sig.plain_called.get(callee_idx).copied().unwrap_or(false) {
            sig.conflict = true;
            return Err(Unsupported::TypeMismatch { pc });
        }
        let key = (callee_idx as u32, identity.clone());
        match sig.specializations.get(&key) {
            Some(&clone) => clone as usize,
            None => {
                // Cap the clone count per original so a pathological program
                // cannot explode the module (falls back loudly instead).
                const MAX_SPECIALIZATIONS: usize = 8;
                let existing = sig
                    .specializations
                    .keys()
                    .filter(|(orig, _)| *orig == callee_idx as u32)
                    .count();
                if existing >= MAX_SPECIALIZATIONS {
                    sig.conflict = true;
                    return Err(Unsupported::TypeMismatch { pc });
                }
                let clone = sig.param_obs.len() as u32;
                let env_total: usize = identity.iter().flatten().map(|id| id.captures as usize).sum();
                sig.param_obs.push(vec![
                    None;
                    funcs[callee_idx].param_count as usize
                        + env_total
                        + funcs[callee_idx].capture_count as usize
                ]);
                sig.ret_types.push(sig.ret_types[callee_idx]);
                sig.ret_closures.push(None);
                sig.ret_closure_poisoned.push(false);
                sig.lambda_params.push(identity.clone());
                sig.specializations.insert(key, clone);
                sig.pending_clones.push(callee_idx as u32);
                clone as usize
            }
        }
    } else {
        if sig.specialized.get(callee_idx).copied().unwrap_or(false) {
            sig.conflict = true;
            return Err(Unsupported::TypeMismatch { pc });
        }
        if let Some(flag) = sig.plain_called.get_mut(callee_idx) {
            *flag = true;
        }
        callee_idx
    };
    // A summarized callee (its single return is a closure whose captures map
    // to parameters) is consumed statically: the result register is seeded
    // with the closure ref built from this call site's argument values. The
    // effect-free body is never emitted and no call happens at runtime.
    if let Some((lf, srcs)) = sig.ret_closures.get(callee_idx).cloned().flatten() {
        let mut caps = Vec::with_capacity(srcs.len());
        for RetCaptureSrc::Param(k) in srcs {
            let arg_reg = dst_reg.wrapping_add(1).wrapping_add(k as u8);
            let (v, ty) = read_scalar(ssa, insts, arg_reg, block, pc)?;
            if matches!(ty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            caps.push(ClosureCapture::Value(v, ty));
        }
        if (dst_reg as usize) < ssa.reg_count {
            ssa.current_def[block][dst_reg as usize] = None;
        }
        ssa.builtin_regs.insert((block, dst_reg), GlobalRef::Closure(lf, caps));
        return Ok(());
    }
    // Tier 1 bridge call (`docs/llvm/tier1-hybrid.md`): the callee runs on the
    // embedded VM. Arguments must match the recorded scalar marshaling types;
    // the destination register stays *unbound*, so any later use of the result
    // rejects the module (results never cross the bridge in v1).
    if let Some(param_tys) = sig.vm_functions.get(&(callee_idx as u32)).cloned() {
        if !captures.is_empty() {
            return Err(Unsupported::Opcode {
                pc,
                op: Opcode::CallDirect,
            });
        }
        let mut args = Vec::with_capacity(argc);
        for (i, param_ty) in param_tys.iter().enumerate().take(argc) {
            let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
            let (aval, aty) = read_scalar(ssa, insts, arg_reg, block, pc)?;
            if aty != *param_ty {
                return Err(Unsupported::TypeMismatch { pc });
            }
            args.push(aval);
        }
        insts.push(Inst::CallVm {
            func: FuncId(callee_idx as u32),
            args,
        });
        ssa.current_def[block][dst_reg as usize] = None;
        ssa.builtin_regs.remove(&(block, dst_reg));
        return Ok(());
    }
    let mut args = Vec::with_capacity(argc + captures.len());
    let mut env_args: Vec<(ValueId, Ty)> = Vec::new();
    for (i, id) in identity.iter().enumerate() {
        let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
        match *id {
            // Erased zero-capture lambda: nothing is passed at runtime.
            Some(LambdaIdentity { captures: 0, .. }) => continue,
            // Erased capturing closure: its environment (resolved to current
            // cell contents at this call site) travels as hidden trailing
            // arguments, in parameter order.
            Some(_) => {
                let Some(GlobalRef::Closure(_, caps)) = ssa.builtin_ref_at(arg_reg, block) else {
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
                };
                for capture in &caps {
                    let (v, ty) = match capture {
                        ClosureCapture::Cell(cid) => {
                            let slot = ssa.cell_slot(*cid);
                            ssa.read_slot(slot, block, pc)?
                        }
                        ClosureCapture::Value(v, ty) => (*v, *ty),
                    };
                    if matches!(ty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    env_args.push((v, ty));
                }
                continue;
            }
            None => {}
        }
        let (aval, aty) = read_scalar(ssa, insts, arg_reg, block, pc)?;
        // `Str` and container handles pass as `ptr` (arena-owned until
        // exit, so no ownership transfer is involved). `Maybe` carriers
        // and nil stay out of the function ABI.
        if matches!(aty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr) {
            return Err(Unsupported::TypeMismatch { pc });
        }
        // Refine the callee's parameter type from this observed argument.
        if let Some(slot) = sig.param_obs.get_mut(callee_idx).and_then(|p| p.get_mut(i)) {
            match slot {
                None => *slot = Some(aty),
                Some(prev) if *prev != aty => sig.conflict = true,
                Some(_) => {}
            }
        }
        args.push(aval);
    }
    // Hidden trailing arguments, in signature order: the erased closures'
    // environment values first, then the callee's own captures. Their types
    // refine the same monomorphization lattice as visible parameters.
    for (k, &(ev, ety)) in env_args.iter().enumerate() {
        if let Some(slot) = sig.param_obs.get_mut(callee_idx).and_then(|p| p.get_mut(argc + k)) {
            match slot {
                None => *slot = Some(ety),
                Some(prev) if *prev != ety => sig.conflict = true,
                Some(_) => {}
            }
        }
        args.push(ev);
    }
    let env_total = env_args.len();
    for (k, &(cval, cty)) in captures.iter().enumerate() {
        if let Some(slot) = sig
            .param_obs
            .get_mut(callee_idx)
            .and_then(|p| p.get_mut(argc + env_total + k))
        {
            match slot {
                None => *slot = Some(cty),
                Some(prev) if *prev != cty => sig.conflict = true,
                Some(_) => {}
            }
        }
        args.push(cval);
    }
    let ret = sig.ret_types.get(callee_idx).copied().unwrap_or(Ty::I64);
    if ret == Ty::Nil {
        insts.push(Inst::CallFn {
            dst: None,
            func: FuncId(callee_idx as u32),
            args,
        });
        let nil = ssa.new_val();
        insts.push(Inst::Const {
            dst: nil,
            value: Const::Nil,
        });
        ssa.write(dst_reg, block, (nil, Ty::Nil));
    } else {
        let dst = ssa.new_val();
        insts.push(Inst::CallFn {
            dst: Some(dst),
            func: FuncId(callee_idx as u32),
            args,
        });
        ssa.write(dst_reg, block, (dst, ret));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn lower_inst(
    ssa: &mut Ssa,
    block: usize,
    insts: &mut Vec<Inst>,
    func: &FunctionData,
    funcs: &[FunctionData],
    entry: u32,
    globals: &mut Vec<String>,
    module_globals: &[String],
    sig: &mut SigInfer,
    capture_params: &[(ValueId, Ty)],
    instr: &Instr,
    pc: usize,
) -> Result<(), Unsupported> {
    match instr.opcode() {
        Opcode::LoadInt => {
            let value = *func
                .consts
                .ints
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::I64(value),
            });
            ssa.const_int.insert(dst, value);
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        Opcode::LoadFloat => {
            let value = *func
                .consts
                .floats
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::F64(value),
            });
            ssa.write(instr.a(), block, (dst, Ty::F64));
        }
        Opcode::LoadBool => {
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Bool(instr.b() != 0),
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::LoadNil => {
            let dst = ssa.new_val();
            insts.push(Inst::Const { dst, value: Const::Nil });
            ssa.write(instr.a(), block, (dst, Ty::Nil));
        }
        Opcode::Move => {
            // A register holding a global ref (no SSA value) propagates the ref
            // instead of a value: the compiler moves the callee into the
            // call-window base before the `Call`. Cross-block backtracking
            // covers refs inherited from predecessors (an SSA definition in
            // this block shadows; conflicting paths resolve to None).
            if let Some(global_ref) = ssa.builtin_ref_at(instr.b(), block) {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
                return Ok(());
            }
            let src = ssa.read(instr.b(), block, pc)?;
            ssa.write(instr.a(), block, src);
        }
        Opcode::Move2 => {
            // Fused adjacent moves: `a ← b`, then `b ← c`. The VM reads `b`
            // before overwriting it; SSA reads naturally see the old value.
            if let Some(global_ref) = ssa.builtin_ref_at(instr.b(), block) {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
            } else {
                let first = ssa.read(instr.b(), block, pc)?;
                ssa.write(instr.a(), block, first);
            }
            if let Some(global_ref) = ssa.builtin_ref_at(instr.c(), block) {
                ssa.builtin_regs.insert((block, instr.b()), global_ref);
            } else {
                let second = ssa.read(instr.c(), block, pc)?;
                ssa.write(instr.b(), block, second);
            }
        }
        Opcode::IsNil => {
            // `a` = dst, `b` = src. The statically-typed subset resolves most nil
            // tests at lower time: concrete scalars are never nil, `Nil` always
            // is; a Maybe carrier (dynamic map/list read) tests its present bit.
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            let dst = ssa.new_val();
            match ty {
                Ty::Nil => insts.push(Inst::Const {
                    dst,
                    value: Const::Bool(true),
                }),
                Ty::I64 | Ty::F64 | Ty::Bool | Ty::Str => insts.push(Inst::Const {
                    dst,
                    value: Const::Bool(false),
                }),
                Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                    let present = ssa.new_val();
                    insts.push(Inst::MaybePresent {
                        dst: present,
                        src: v,
                        maybe_ty: ty,
                    });
                    insts.push(Inst::Not { dst, src: present });
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::IsList => {
            // `a` = dst, `b` = src. In the statically-typed subset the list-ness
            // of a register is known at lower time: a typed list handle is a
            // list; every other lowerable type (scalars, maps, maybe-carriers,
            // nil) is not. Const-folds to a `Bool`, mirroring the VM's
            // `runtime_value_is_list`.
            let (_, ty) = ssa.read(instr.b(), block, pc)?;
            let is_list = matches!(ty, Ty::ListI64 | Ty::ListF64 | Ty::ListStr);
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Bool(is_list),
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::IsMap => {
            // `a` = dst, `b` = src. Analogous to `IsList`: a typed map handle is
            // a map at lower time; every other lowerable type is not. Const-folds
            // to a `Bool`, mirroring the VM's `runtime_value_is_map`.
            let (_, ty) = ssa.read(instr.b(), block, pc)?;
            let is_map = matches!(
                ty,
                Ty::MapStrI64 | Ty::MapI64I64 | Ty::MapStrF64 | Ty::MapI64F64 | Ty::MapStrBool
            );
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Bool(is_map),
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::Not => {
            // `!x`: `a` = dst, `b` = src. The VM negates a `Bool` and treats `Nil` as
            // `true`; a non-bool/non-nil operand is a VM error, so reject (fall back).
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            let dst = ssa.new_val();
            match ty {
                Ty::Bool => insts.push(Inst::Not { dst, src: v }),
                Ty::Nil => insts.push(Inst::Const {
                    dst,
                    value: Const::Bool(true),
                }),
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        op @ (Opcode::AddInt | Opcode::SubInt | Opcode::MulInt | Opcode::DivInt | Opcode::ModInt) => {
            // These opcodes dispatch on runtime operand type in the VM: two ints →
            // integer op; any float operand → coerce ints to float and use the float
            // op (matching `dynamic_add`/etc.). We resolve that dispatch statically.
            // A `Maybe` operand (dynamic index result) unwraps to `I64` here.
            //
            // A Dyn operand routes both sides through the `dyn.*` helpers,
            // which carry the same promotion rules at runtime (`/` always
            // Float, type errors abort like the VM). Result stays `Ty::Dyn`.
            {
                let (lv_raw, lty_raw) = ssa.read(instr.b(), block, pc)?;
                let (rv_raw, rty_raw) = ssa.read(instr.c(), block, pc)?;
                if lty_raw == Ty::Dyn || rty_raw == Ty::Dyn {
                    let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                    let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                    let helper = match op {
                        Opcode::AddInt => "add",
                        Opcode::SubInt => "sub",
                        Opcode::MulInt => "mul",
                        Opcode::DivInt => "div",
                        _ => "mod",
                    };
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("dyn", helper),
                        args: vec![lhs, rhs],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Dyn));
                    return Ok(());
                }
            }
            let (lv, lty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            let (rv, rty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
            match (lty, rty) {
                (Ty::I64, Ty::I64) => {
                    let dst = ssa.new_val();
                    insts.push(Inst::IntBin {
                        dst,
                        op: int_bin_op(op),
                        lhs: lv,
                        rhs: rv,
                    });
                    ssa.write(instr.a(), block, (dst, Ty::I64));
                }
                (Ty::F64, Ty::F64) | (Ty::I64, Ty::F64) | (Ty::F64, Ty::I64) => {
                    let lhs = coerce_to_f64(ssa, insts, lv, lty);
                    let rhs = coerce_to_f64(ssa, insts, rv, rty);
                    let dst = ssa.new_val();
                    insts.push(Inst::FloatBin {
                        dst,
                        op: int_to_float_bin_op(op),
                        lhs,
                        rhs,
                    });
                    ssa.write(instr.a(), block, (dst, Ty::F64));
                }
                // `str + str` is concatenation (the VM's `AddInt` dispatches to it);
                // only `+` is defined on strings — `-`/`*`/… are VM errors, so reject.
                (Ty::Str, Ty::Str) if matches!(op, Opcode::AddInt) => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("str", "concat"),
                        args: vec![lv, rv],
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Str));
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
        }
        Opcode::NewList => {
            // `a` = dst, `b` = base, `c` = count: a register-window list. The
            // compiler also uses this to box method-call arguments, so the raw
            // elements are always recorded as an ArgList ref; a homogeneous
            // scalar window additionally materializes a real list handle.
            let count = instr.c() as usize;
            let mut elems = Vec::with_capacity(count);
            for i in 0..count {
                let reg = instr.b().wrapping_add(i as u8);
                elems.push(ssa.read(reg, block, pc)?);
            }
            let all = |t: Ty| elems.iter().all(|&(_, ty)| ty == t);
            let materialized = if !elems.is_empty() && all(Ty::I64) {
                Some(("i64_new", "i64_push", Ty::ListI64))
            } else if !elems.is_empty() && all(Ty::F64) {
                Some(("f64_new", "f64_push", Ty::ListF64))
            } else if !elems.is_empty() && all(Ty::Str) {
                Some(("str_new", "str_push", Ty::ListStr))
            } else {
                None
            };
            if let Some((new_fn, push_fn, list_ty)) = materialized {
                let handle = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(handle),
                    callee: AbiRef::new("list_h", new_fn),
                    args: Vec::new(),
                });
                for &(v, _) in &elems {
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", push_fn),
                        args: vec![handle, v],
                    });
                }
                ssa.list_len.insert(handle, elems.len() as i64);
                ssa.list_base_len.insert(handle, elems.len() as i64);
                ssa.write(instr.a(), block, (handle, list_ty));
            }
            // Recorded after the write (which clears the slot) so both views
            // coexist: SSA reads see the handle, method dispatch sees elements.
            ssa.builtin_regs.insert((block, instr.a()), GlobalRef::ArgList(elems));
        }
        Opcode::GetIndexStrI | Opcode::SetIndexStrI => {
            // Composite string-int key access (`m["n${i}"]`): the key is the
            // compiler-proven constant prefix plus the decimal suffix register.
            // A store passes (prefix, suffix) straight to the `set_ik` ABI (key
            // built on the stack inside lkrt, nothing to free); a load builds
            // the key via `concat_i64` in one allocation, and the fresh
            // temporary frees right after the map call.
            let Some(key_fact) = func.performance.known_key(pc).and_then(|fact| fact.string_int) else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            let prefix = func
                .consts
                .strings
                .get(key_fact.prefix_key as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let prefix_v = materialize_key(ssa, insts, globals, prefix);
            let is_set = instr.opcode() == Opcode::SetIndexStrI;
            let (map_reg, suffix_reg) = if is_set {
                (instr.a(), instr.b())
            } else {
                (instr.b(), instr.c())
            };
            let (handle, map_ty) = ssa.read(map_reg, block, pc)?;
            let suffix = ssa.read_typed(suffix_reg, block, Ty::I64, pc)?;
            if is_set {
                let (value, value_ty) = ssa.read(instr.c(), block, pc)?;
                let set_fn = match (map_ty, value_ty) {
                    (Ty::MapStrI64, Ty::I64) => "str_i64_set_ik",
                    (Ty::MapStrF64, Ty::F64) => "str_f64_set_ik",
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", set_fn),
                    args: vec![handle, prefix_v, suffix, value],
                });
            } else {
                let key = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(key),
                    callee: AbiRef::new("str", "concat_i64"),
                    args: vec![prefix_v, suffix],
                });
                let dst = ssa.new_val();
                let maybe_ty = match map_ty {
                    Ty::MapStrI64 => {
                        insts.push(Inst::MapGetMaybe { dst, handle, key });
                        Ty::MaybeI64
                    }
                    Ty::MapStrF64 => {
                        insts.push(Inst::MapGetMaybeStrF64 { dst, handle, key });
                        Ty::MaybeF64
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                free_owned_str(insts, key);
                ssa.write(instr.a(), block, (dst, maybe_ty));
            }
        }
        Opcode::MidInt => {
            // `a = (b + c) / 2` — wrapping add then truncated division
            // (`wrapping_add / 2` in the VM; the guarded div helper's
            // `wrapping_div` by the constant 2 is identical).
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let rhs = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let sum = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: sum,
                op: IntBinOp::Add,
                lhs,
                rhs,
            });
            let two = ssa.new_val();
            insts.push(Inst::Const {
                dst: two,
                value: Const::I64(2),
            });
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: IntBinOp::Div,
                lhs: sum,
                rhs: two,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        Opcode::MinInt | Opcode::MaxInt => {
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let rhs = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let op = if instr.opcode() == Opcode::MinInt {
                IntBinOp::Min
            } else {
                IntBinOp::Max
            };
            let dst = ssa.new_val();
            insts.push(Inst::IntBin { dst, op, lhs, rhs });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        op @ (Opcode::AddMulInt | Opcode::Add2Int) => {
            // Fused accumulator updates: `a += b * c` / `a += b + c` (all Int,
            // wrapping — matching the VM's typed fast path, which bails on
            // non-Int operands, so the static Int requirement is exact).
            let acc = read_typed_scalar(ssa, insts, instr.a(), block, Ty::I64, pc)?;
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let rhs = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let term = ssa.new_val();
            insts.push(Inst::IntBin {
                dst: term,
                op: if op == Opcode::AddMulInt {
                    IntBinOp::Mul
                } else {
                    IntBinOp::Add
                },
                lhs,
                rhs,
            });
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: IntBinOp::Add,
                lhs: acc,
                rhs: term,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        op @ (Opcode::AddListInt | Opcode::SubListInt) => {
            // `a ±= list[key]` — the element read has the VM's exact
            // negative-index/halt semantics via the scalar list read.
            let acc = read_typed_scalar(ssa, insts, instr.a(), block, Ty::I64, pc)?;
            let item = list_i64_element_scalar(ssa, insts, instr.b(), instr.c(), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: if op == Opcode::AddListInt {
                    IntBinOp::Add
                } else {
                    IntBinOp::Sub
                },
                lhs: acc,
                rhs: item,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        op @ (Opcode::AddIntI | Opcode::MulIntI | Opcode::ModIntI) => {
            let lhs = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            let imm = ssa.new_val();
            insts.push(Inst::Const {
                dst: imm,
                value: Const::I64(instr.sc() as i64),
            });
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: imm_int_bin_op(op),
                lhs,
                rhs: imm,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        op @ (Opcode::AddFloat | Opcode::SubFloat | Opcode::MulFloat | Opcode::DivFloat | Opcode::ModFloat) => {
            // The compiler emits these when it expects float arithmetic, but an
            // operand may still be an `I64` (e.g. an `I64` parameter in `x / 2.0`) —
            // the VM coerces it, so we widen `I64`/`Maybe` operands to `F64` here.
            let (lv, lty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            let (rv, rty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
            if !matches!(lty, Ty::I64 | Ty::F64) || !matches!(rty, Ty::I64 | Ty::F64) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let lhs = coerce_to_f64(ssa, insts, lv, lty);
            let rhs = coerce_to_f64(ssa, insts, rv, rty);
            let dst = ssa.new_val();
            insts.push(Inst::FloatBin {
                dst,
                op: float_bin_op(op),
                lhs,
                rhs,
            });
            ssa.write(instr.a(), block, (dst, Ty::F64));
        }
        op @ (Opcode::CmpInt
        | Opcode::CmpNeInt
        | Opcode::CmpLtInt
        | Opcode::CmpLeInt
        | Opcode::CmpGtInt
        | Opcode::CmpGeInt) => {
            // Like arithmetic, comparisons dispatch on runtime operand type: two
            // ints → integer compare; any float operand → float compare (coercing);
            // two strings → a `strcmp`-style helper compared to 0. A `Maybe` operand
            // (dynamic index result) unwraps to `I64` here.
            //
            // `== nil` / `!= nil` resolves *before* the scalar read (which would
            // unwrap a Maybe, aborting on absent): a Maybe operand tests its
            // present bit, a concrete-typed operand folds to a constant (values
            // of non-Maybe types are never nil). Ordered nil comparisons are VM
            // errors, so they reject.
            let (lv_raw, lty_raw) = ssa.read(instr.b(), block, pc)?;
            let (rv_raw, rty_raw) = ssa.read(instr.c(), block, pc)?;
            if lty_raw == Ty::Nil || rty_raw == Ty::Nil {
                let cop = cmp_op(op);
                if !matches!(cop, CmpOp::Eq | CmpOp::Ne) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                let (other_v, other_ty) = if lty_raw == Ty::Nil {
                    (rv_raw, rty_raw)
                } else {
                    (lv_raw, lty_raw)
                };
                match other_ty {
                    Ty::Nil => {
                        let dst = ssa.new_val();
                        insts.push(Inst::Const {
                            dst,
                            value: Const::Bool(cop == CmpOp::Eq),
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Bool));
                    }
                    Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                        let present = ssa.new_val();
                        insts.push(Inst::MaybePresent {
                            dst: present,
                            src: other_v,
                            maybe_ty: other_ty,
                        });
                        if cop == CmpOp::Ne {
                            ssa.write(instr.a(), block, (present, Ty::Bool));
                        } else {
                            let dst = ssa.new_val();
                            insts.push(Inst::Not { dst, src: present });
                            ssa.write(instr.a(), block, (dst, Ty::Bool));
                        }
                    }
                    // A boxed Dyn: nil-ness is its tag (`0` = Nil).
                    Ty::Dyn => {
                        let tag = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(tag),
                            callee: AbiRef::new("dyn", "tag"),
                            args: vec![other_v],
                        });
                        let zero = ssa.new_val();
                        insts.push(Inst::Const {
                            dst: zero,
                            value: Const::I64(0),
                        });
                        let dst = ssa.new_val();
                        insts.push(Inst::Cmp {
                            dst,
                            op: if cop == CmpOp::Eq { CmpOp::Eq } else { CmpOp::Ne },
                            float: false,
                            lhs: tag,
                            rhs: zero,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Bool));
                    }
                    _ => {
                        let dst = ssa.new_val();
                        insts.push(Inst::Const {
                            dst,
                            value: Const::Bool(cop == CmpOp::Ne),
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Bool));
                    }
                }
                return Ok(());
            }
            // A Dyn operand: box the other side and compare through the
            // `dyn.*` helpers (VM equality semantics live in lkrt; ordered
            // compares are numeric-only there, aborting like the VM).
            if lty_raw == Ty::Dyn || rty_raw == Ty::Dyn {
                let lhs = to_dyn(ssa, insts, lv_raw, lty_raw, pc)?;
                let rhs = to_dyn(ssa, insts, rv_raw, rty_raw, pc)?;
                let (helper, negate) = match cmp_op(op) {
                    CmpOp::Eq => ("eq", false),
                    CmpOp::Ne => ("eq", true),
                    CmpOp::Lt => ("lt", false),
                    CmpOp::Le => ("le", false),
                    CmpOp::Gt => ("gt", false),
                    CmpOp::Ge => ("ge", false),
                };
                let raw = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(raw),
                    callee: AbiRef::new("dyn", helper),
                    args: vec![lhs, rhs],
                });
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::I64(0),
                });
                let dst = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst,
                    op: if negate { CmpOp::Eq } else { CmpOp::Ne },
                    float: false,
                    lhs: raw,
                    rhs: zero,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            let (lv, lty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            let (rv, rty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
            let (float, lhs, rhs) = match (lty, rty) {
                (Ty::I64, Ty::I64) => (false, lv, rv),
                // Bool equality (`b == true`): widen to i64 (the integer
                // compare renders `icmp … i64`); ordered comparisons on Bools
                // are VM errors, so they reject.
                (Ty::Bool, Ty::Bool) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let lw = ssa.new_val();
                    insts.push(Inst::ZextBool { dst: lw, src: lv });
                    let rw = ssa.new_val();
                    insts.push(Inst::ZextBool { dst: rw, src: rv });
                    (false, lw, rw)
                }
                (Ty::F64, Ty::F64) | (Ty::I64, Ty::F64) | (Ty::F64, Ty::I64) => (
                    true,
                    coerce_to_f64(ssa, insts, lv, lty),
                    coerce_to_f64(ssa, insts, rv, rty),
                ),
                // List structural equality: same length + element-wise `==` via
                // an lkrt helper returning 1/0, compared against 1 (so `!=`
                // reuses the same op). Int/Float lists compare with numeric
                // coercion (`[1] == [1.0]` is true); other cross-typed pairs
                // reject — folding them to `false` would be wrong for two
                // empty lists, which the VM deems equal regardless of type.
                (Ty::ListI64, Ty::ListI64)
                | (Ty::ListF64, Ty::ListF64)
                | (Ty::ListStr, Ty::ListStr)
                | (Ty::ListI64, Ty::ListF64)
                | (Ty::ListF64, Ty::ListI64) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let (helper, a, b) = match (lty, rty) {
                        (Ty::ListI64, Ty::ListI64) => ("i64_eq", lv, rv),
                        (Ty::ListF64, Ty::ListF64) => ("f64_eq", lv, rv),
                        (Ty::ListStr, Ty::ListStr) => ("str_eq", lv, rv),
                        // The mixed helper takes (ints, floats).
                        (Ty::ListI64, Ty::ListF64) => ("i64_f64_eq", lv, rv),
                        _ => ("i64_f64_eq", rv, lv),
                    };
                    let eq = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(eq),
                        callee: AbiRef::new("list_h", helper),
                        args: vec![a, b],
                    });
                    let one = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: one,
                        value: Const::I64(1),
                    });
                    (false, eq, one)
                }
                // Cross-typed list pairs beyond Int/Float can only be equal
                // when *both* are empty (the VM compares structurally
                // regardless of the typed-list representation). With both
                // proven non-empty at materialization (lengths never shrink),
                // the comparison folds; an unproven side could be empty at
                // runtime, so it rejects instead of guessing.
                (Ty::ListI64 | Ty::ListF64 | Ty::ListStr, Ty::ListI64 | Ty::ListF64 | Ty::ListStr) => {
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let lbase = ssa.list_base_len.get(&lv).copied().unwrap_or(0);
                    let rbase = ssa.list_base_len.get(&rv).copied().unwrap_or(0);
                    if lbase < 1 || rbase < 1 {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let dst = ssa.new_val();
                    insts.push(Inst::Const {
                        dst,
                        value: Const::Bool(cmp_op(op) == CmpOp::Ne),
                    });
                    ssa.write(instr.a(), block, (dst, Ty::Bool));
                    return Ok(());
                }
                (Ty::Str, Ty::Str) => {
                    // The VM only supports `==`/`!=` on strings (ordered comparisons
                    // are a runtime error), so reject the rest — falling back rather
                    // than computing an order the VM would refuse.
                    if !matches!(cmp_op(op), CmpOp::Eq | CmpOp::Ne) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    // `str_cmp(a, b)` returns -1/0/1; comparing to 0 realizes `==`/`!=`.
                    let cmp = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(cmp),
                        callee: AbiRef::new("str", "cmp"),
                        args: vec![lv, rv],
                    });
                    let zero = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: zero,
                        value: Const::I64(0),
                    });
                    (false, cmp, zero)
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let dst = ssa.new_val();
            insts.push(Inst::Cmp {
                dst,
                op: cmp_op(op),
                float,
                lhs,
                rhs,
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::CallMethodK => {
            lower_method_call_k(ssa, insts, globals, func, funcs, entry, sig, instr, block, pc)?;
        }
        Opcode::CallDirect => {
            // Register-window call: `a`=dst register, `b`=callee function index,
            // `c`=argument count; the args occupy registers `[a+1, a+1+c)`. Each
            // argument's observed scalar type refines the callee's parameter type
            // (`sig.param_obs`); disagreeing sites mark the callee polymorphic
            // (`sig.conflict` → whole-module fallback). The result takes the callee's
            // inferred return type, so `f64`/`bool`-returning calls type correctly.
            let callee_idx = instr.b() as usize;
            lower_user_call(
                ssa,
                insts,
                funcs,
                entry,
                sig,
                callee_idx,
                instr.a(),
                instr.c() as usize,
                &[],
                block,
                pc,
            )?;
        }
        // Direct calls address the callee by index, so the loaded function
        // value itself only flows into the compiler's global-table storage
        // (`SetGlobal`), which stays a no-op.
        Opcode::LoadFunction => {
            ssa.builtin_regs.insert((block, instr.a()), GlobalRef::UserFn);
        }
        Opcode::MakeClosure => {
            // `a` = dst, `b` = function index, `c` = capture window base. A
            // zero-capture closure is a statically known function reference; a
            // capturing one additionally snapshots the capture window by value
            // (exactly the VM's `capture_values` copy) — the values become
            // hidden trailing arguments at each call. Mutable captures compile
            // to cells (`LoadCellVal`/`StoreCellVal`), which stay unsupported.
            let fidx = instr.b() as usize;
            let callee = funcs.get(fidx).ok_or(Unsupported::BadConst { pc })?;
            if callee.capture_count == 0 {
                ssa.builtin_regs
                    .insert((block, instr.a()), GlobalRef::Lambda(fidx as u32));
                return Ok(());
            }
            let mut captures = Vec::with_capacity(callee.capture_count as usize);
            for k in 0..callee.capture_count {
                let reg = instr.c().wrapping_add(k as u8);
                // The compiler captures locals through upvalue cells (shared
                // mutable boxes); a plain value is captured directly.
                if let Some(GlobalRef::Cell(cid)) = ssa.builtin_regs.get(&(block, reg)) {
                    captures.push(ClosureCapture::Cell(*cid));
                    continue;
                }
                let (v, ty) = ssa.read(reg, block, pc)?;
                // Same set as call arguments: scalars and handles pass through,
                // `Maybe`/nil carriers stay out of the function ABI.
                if matches!(ty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
                    return Err(Unsupported::TypeMismatch { pc });
                }
                captures.push(ClosureCapture::Value(v, ty));
            }
            ssa.builtin_regs
                .insert((block, instr.a()), GlobalRef::Closure(fidx as u32, captures));
        }
        Opcode::LoadCapture => {
            // `a` = dst, `bx` = capture index. Captures are cells: the loaded
            // register carries a cell ref whose `LoadCellVal` reads the hidden
            // trailing parameter (the cell's value at the call site). A direct
            // (non-cell) use of the register finds no SSA value and rejects.
            let k = instr.bx() as usize;
            if k >= capture_params.len() {
                return Err(Unsupported::BadConst { pc });
            }
            ssa.builtin_regs.insert((block, instr.a()), GlobalRef::CellParam(k));
        }
        Opcode::LoadCellVal => {
            // `a` = dst, `b` = cell register: reads the cell's current content.
            // The cell ref backtracks across blocks like any global ref; the
            // content read goes through the virtual slot (phis on demand).
            match ssa.builtin_ref_at(instr.b(), block) {
                Some(GlobalRef::CellParam(k)) => {
                    let &(v, ty) = capture_params.get(k).ok_or(Unsupported::BadConst { pc })?;
                    ssa.write(instr.a(), block, (v, ty));
                }
                Some(GlobalRef::Cell(cid)) => {
                    let slot = ssa.cell_slot(cid);
                    let (v, ty) = ssa.read_slot(slot, block, pc)?;
                    ssa.write(instr.a(), block, (v, ty));
                }
                _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            }
        }
        Opcode::StoreCellVal => {
            // `a` = cell register, `b` = value register: updates the tracked
            // cell content (mutations inside a lambda would need write-back
            // through the hidden parameter, so `CellParam` stores reject).
            let Some(GlobalRef::Cell(cid)) = ssa.builtin_ref_at(instr.a(), block) else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            let slot = ssa.cell_slot(cid);
            ssa.write_slot(slot, block, (v, ty));
        }
        Opcode::SetGlobal => {
            // Storing a function value into the global table is the compiler's
            // top-level `fn` bookkeeping — a no-op natively.
            if let Some(GlobalRef::UserFn) = ssa.builtin_regs.get(&(block, instr.a())) {
                return Ok(());
            }
            // A top-level `let f = |x| …` stores a lambda ref: a no-op when the
            // prescan proved the slot single-assigned with this exact closure
            // (readers resolve it statically); anything else would let readers
            // observe a stale ref, so it rejects.
            if let Some(GlobalRef::Lambda(fidx)) = ssa.builtin_regs.get(&(block, instr.a())) {
                if sig.lambda_globals.get(instr.bx() as usize).copied().flatten() == Some(*fidx) {
                    return Ok(());
                }
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            }
            // Writing a global whose *name* this lowering recognizes would let
            // later `GetGlobal` reads resolve to the stale builtin/module
            // meaning and miscompile (`println = f; println(x)`), so those
            // writes reject the program.
            let slot = instr.bx();
            let name = module_globals.get(slot as usize).map(String::as_str);
            if let Some(name) = name
                && (matches!(
                    name,
                    "println" | "print" | "assert" | "assert_eq" | "assert_ne" | "panic" | "typeof"
                ) || MODULE_GLOBALS.contains(&name))
            {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            }
            // Mutable scalar module global (a top-level `let` shared with
            // functions): the write's type must agree across the whole module.
            let (v, ty) = ssa.read(instr.a(), block, pc)?;
            if !matches!(ty, Ty::I64 | Ty::F64 | Ty::Bool | Ty::Str) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            match sig.global_tys.get_mut(slot as usize) {
                Some(state @ None) => *state = Some(ty),
                Some(Some(prev)) if *prev != ty => return Err(Unsupported::TypeMismatch { pc }),
                Some(Some(_)) => {}
                None => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
            }
            insts.push(Inst::GlobalSet {
                gvar: sig.gvar(slot),
                src: v,
            });
        }
        Opcode::GetGlobal => {
            // Reads of known runtime builtins / stdlib module objects load a
            // global ref (no SSA value — any unrecognized use finds it
            // undefined and rejects). Reads of mutable scalar globals load the
            // typed value, but only for slots provably initialized in the
            // entry prefix (the VM's pre-write value is nil, native storage is
            // zero — a read that could observe it must reject).
            let slot = instr.bx();
            let name = module_globals.get(slot as usize).map(String::as_str);
            let global_ref = match name {
                Some("println") => Some(GlobalRef::Builtin(Builtin::Println)),
                Some("print") => Some(GlobalRef::Builtin(Builtin::Print)),
                Some("assert") => Some(GlobalRef::Builtin(Builtin::Assert)),
                Some("assert_eq") => Some(GlobalRef::Builtin(Builtin::AssertEq)),
                Some("assert_ne") => Some(GlobalRef::Builtin(Builtin::AssertNe)),
                Some("panic") => Some(GlobalRef::Builtin(Builtin::Panic)),
                Some("typeof") => Some(GlobalRef::Builtin(Builtin::Typeof)),
                Some("__lk_call_method") => Some(GlobalRef::Builtin(Builtin::CallMethod)),
                Some(name) if MODULE_GLOBALS.contains(&name) => Some(GlobalRef::Module(name.to_string())),
                _ => None,
            };
            if let Some(global_ref) = global_ref {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
                return Ok(());
            }
            // A single-assignment top-level lambda slot resolves statically to
            // its function reference (initialization-order safe: the prescan
            // only accepts entry-prefix writes, which precede any user call).
            if let Some(fidx) = sig.lambda_globals.get(slot as usize).copied().flatten() {
                ssa.builtin_regs.insert((block, instr.a()), GlobalRef::Lambda(fidx));
                return Ok(());
            }
            let initialized = sig.initialized_globals.get(slot as usize).copied().unwrap_or(false);
            let ty = sig.global_tys.get(slot as usize).copied().flatten();
            let (Some(ty), true) = (ty, initialized) else {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            };
            let dst = ssa.new_val();
            insts.push(Inst::GlobalGet {
                dst,
                gvar: sig.gvar(slot),
            });
            ssa.write(instr.a(), block, (dst, ty));
        }
        Opcode::Call => {
            // Register-window call: `a` = window base (the callee slot), `c` =
            // positional count, args at `[a+1, a+1+c)`. Only calls whose callee
            // register holds a recognized global ref lower; everything else
            // (closures, runtime values) rejects.
            let base = instr.a();
            match ssa.builtin_ref_at(base, block) {
                Some(GlobalRef::Builtin(Builtin::CallMethod)) => {
                    if instr.c() != 3 {
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                    }
                    lower_method_call(ssa, insts, globals, base, block, pc)?;
                }
                Some(GlobalRef::Builtin(builtin)) => {
                    lower_builtin_call(ssa, insts, globals, builtin, base, instr.c() as usize, block, pc)?;
                }
                Some(GlobalRef::ModuleFn(module, name)) => {
                    lower_module_call(ssa, insts, &module, &name, base, instr.c() as usize, block, pc)?;
                }
                // An indirect call through a statically known capture-free
                // closure devirtualizes to a direct call (same register-window
                // layout: result at `base`, args at `[base+1, base+1+c)`).
                Some(GlobalRef::Lambda(fidx)) => {
                    lower_user_call(
                        ssa,
                        insts,
                        funcs,
                        entry,
                        sig,
                        fidx as usize,
                        base,
                        instr.c() as usize,
                        &[],
                        block,
                        pc,
                    )?;
                }
                // A capturing closure devirtualizes the same way; each cell
                // capture resolves to the cell's content *at this call site*
                // (the VM's shared-mutable-cell semantics) and is appended as
                // a hidden trailing argument.
                Some(GlobalRef::Closure(fidx, captures)) => {
                    let mut resolved = Vec::with_capacity(captures.len());
                    for capture in &captures {
                        let (v, ty) = match capture {
                            ClosureCapture::Cell(cid) => {
                                let slot = ssa.cell_slot(*cid);
                                ssa.read_slot(slot, block, pc)?
                            }
                            ClosureCapture::Value(v, ty) => (*v, *ty),
                        };
                        if matches!(ty, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool) {
                            return Err(Unsupported::TypeMismatch { pc });
                        }
                        resolved.push((v, ty));
                    }
                    lower_user_call(
                        ssa,
                        insts,
                        funcs,
                        entry,
                        sig,
                        fidx as usize,
                        base,
                        instr.c() as usize,
                        &resolved,
                        block,
                        pc,
                    )?;
                }
                Some(GlobalRef::Module(_))
                | Some(GlobalRef::UserFn)
                | Some(GlobalRef::ArgList(_))
                | Some(GlobalRef::Cell(_))
                | Some(GlobalRef::CellParam(_))
                | None => {
                    return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                }
            }
        }
        // A string constant materializes an interned module global (a C-string) with
        // type `Str`. String *operations* (concat/compare/…) aren't modelled, so a
        // `Str` flowing into one rejects (falls back); but a returned literal prints,
        // and a constant map key is consumed directly by the map ABI.
        Opcode::LoadString => {
            let s = func
                .consts
                .strings
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let gid = intern_global(globals, s);
            let dst = ssa.new_val();
            insts.push(Inst::Const {
                dst,
                value: Const::Str(GlobalId(gid)),
            });
            ssa.const_strs.insert(dst, s.clone());
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::ToString => {
            // `a` = dst, `b` = source. Display-convert to a `Str` (Str/Int/Bool
            // supported; float/other fall back).
            let (v, ty) = ssa.read(instr.b(), block, pc)?;
            // The result is register-visible, so it stays arena-owned (never
            // freed eagerly, reclaimed by `lkrt_cleanup` at exit).
            let (s, _fresh) = to_display_str(ssa, insts, globals, v, ty, false, pc)?;
            ssa.write(instr.a(), block, (s, Ty::Str));
        }
        Opcode::ConcatString => {
            // `a` = dst, `b` = lhs, `c` = rhs. Concatenate `display(lhs) ++
            // display(rhs)` (each operand display-converted, as the VM does);
            // an int rhs fuses into a single `concat_i64` call.
            let (lv, lty) = ssa.read(instr.b(), block, pc)?;
            let (rv, rty) = ssa.read(instr.c(), block, pc)?;
            let (l, l_fresh) = to_display_str(ssa, insts, globals, lv, lty, false, pc)?;
            let dst = concat_display(ssa, insts, globals, l, rv, rty, false, pc)?;
            if l_fresh {
                free_owned_str(insts, l);
            }
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::ConcatN => {
            // `a` = dst, `b` = first element register, `c` = element count. The VM
            // display-converts each element then concatenates; each element is
            // display-converted (`Str`/`Int`/`Bool`) and folded via repeated
            // `str_concat` — int elements fuse into `concat_i64` (no suffix
            // temporary). A float/other element rejects (falls back).
            let start = instr.b();
            let count = instr.c() as usize;
            let result = if count == 0 {
                // Empty concat → the empty string.
                let gid = intern_global(globals, "");
                let dst = ssa.new_val();
                insts.push(Inst::Const {
                    dst,
                    value: Const::Str(GlobalId(gid)),
                });
                dst
            } else {
                let (v0, ty0) = ssa.read(start, block, pc)?;
                let (mut acc, mut acc_fresh) = to_display_str(ssa, insts, globals, v0, ty0, false, pc)?;
                for i in 1..count {
                    let (v, ty) = ssa.read(start.wrapping_add(i as u8), block, pc)?;
                    let dst = concat_display(ssa, insts, globals, acc, v, ty, false, pc)?;
                    // The consumed accumulator is dead; free it if this
                    // lowering allocated it.
                    if acc_fresh {
                        free_owned_str(insts, acc);
                    }
                    acc = dst;
                    acc_fresh = true;
                }
                acc
            };
            ssa.write(instr.a(), block, (result, Ty::Str));
        }
        Opcode::ListJoin => {
            // `a` = dst, `b` = list, `c` = separator. The VM joins a *string* list; we
            // support `List<str>` with a `Str` separator → a fresh `Str`.
            let (handle, list_ty) = ssa.read(instr.b(), block, pc)?;
            if list_ty != Ty::ListStr {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let sep = ssa.read_typed(instr.c(), block, Ty::Str, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "str_join"),
                args: vec![handle, sep],
            });
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::LoadHeapConst => {
            // Constant container literals: materialize a growable `lkrt` handle.
            //  - `List<i64>` / `List<f64>` → new + push per element.
            //  - `Map<str, i64>` → new + set per (const-string key, int value) entry.
            // Other heap constants (nested/mixed, other key/elem types, long strings)
            // fall back.
            let hv = func
                .consts
                .heap_values
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            match hv {
                ConstHeapValueData::List(elems) => {
                    // An empty `[]` is ambiguous — a lookahead types it from the
                    // first value pushed (a wrong guess only costs a fallback).
                    if elems.is_empty() && empty_list_is_str_elem(&func.code, pc, instr.a()) {
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("list_h", "str_new"),
                            args: Vec::new(),
                        });
                        ssa.list_len.insert(handle, 0);
                        ssa.list_base_len.insert(handle, 0);
                        ssa.write(instr.a(), block, (handle, Ty::ListStr));
                        return Ok(());
                    }
                    let all_int = elems.iter().all(|e| matches!(e, ConstRuntimeValueData::Int(_)));
                    let all_float = elems.iter().all(|e| matches!(e, ConstRuntimeValueData::Float(_)));
                    let all_str = elems.iter().all(|e| matches!(e, ConstRuntimeValueData::ShortStr(_)));
                    let (new_fn, push_fn, list_ty) = if all_int {
                        ("i64_new", "i64_push", Ty::ListI64)
                    } else if all_float {
                        ("f64_new", "f64_push", Ty::ListF64)
                    } else if all_str {
                        ("str_new", "str_push", Ty::ListStr)
                    } else {
                        // Mixed scalar elements: a boxed-dynamic list (plan
                        // M4.2 Dyn). Nested containers still fall back.
                        let scalar_only = elems.iter().all(|e| {
                            matches!(
                                e,
                                ConstRuntimeValueData::Nil
                                    | ConstRuntimeValueData::Bool(_)
                                    | ConstRuntimeValueData::Int(_)
                                    | ConstRuntimeValueData::Float(_)
                                    | ConstRuntimeValueData::ShortStr(_)
                            )
                        });
                        if !scalar_only {
                            return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                        }
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("list_h", "dyn_new"),
                            args: Vec::new(),
                        });
                        for e in elems {
                            let boxed = box_const_scalar(ssa, insts, globals, e);
                            insts.push(Inst::Call {
                                dst: None,
                                callee: AbiRef::new("list_h", "dyn_push"),
                                args: vec![handle, boxed],
                            });
                        }
                        ssa.list_len.insert(handle, elems.len() as i64);
                        ssa.list_base_len.insert(handle, elems.len() as i64);
                        ssa.write(instr.a(), block, (handle, Ty::ListDyn));
                        return Ok(());
                    };
                    let handle = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(handle),
                        callee: AbiRef::new("list_h", new_fn),
                        args: Vec::new(),
                    });
                    for e in elems {
                        let v = ssa.new_val();
                        let value = match e {
                            ConstRuntimeValueData::Int(n) => Const::I64(*n),
                            ConstRuntimeValueData::Float(x) => Const::F64(*x),
                            ConstRuntimeValueData::ShortStr(s) => Const::Str(GlobalId(intern_global(globals, s))),
                            _ => unreachable!("filtered to a single element type above"),
                        };
                        insts.push(Inst::Const { dst: v, value });
                        insts.push(Inst::Call {
                            dst: None,
                            callee: AbiRef::new("list_h", push_fn),
                            args: vec![handle, v],
                        });
                    }
                    ssa.list_len.insert(handle, elems.len() as i64);
                    ssa.list_base_len.insert(handle, elems.len() as i64);
                    ssa.write(instr.a(), block, (handle, list_ty));
                }
                ConstHeapValueData::Map(entries) => {
                    // `Map<str,i64>` / `Map<i64,i64>` / `Map<str,f64>` / `Map<str,bool>`
                    // — keys uniformly string or int, values uniformly int, float, or
                    // bool. Other shapes (mixed, non-scalar values) fall back.
                    let all_bool_vals =
                        !entries.is_empty() && entries.iter().all(|(_, v)| matches!(v, ConstRuntimeValueData::Bool(_)));
                    let all_str_keys = entries
                        .iter()
                        .all(|(k, _)| matches!(k, RuntimeMapKeyData::ShortStr(_) | RuntimeMapKeyData::String(_)));
                    if all_bool_vals && all_str_keys {
                        // Values ride the str_i64 map ABI as 0/1; the MapStrBool
                        // type keeps display/compare semantics exact.
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("map_h", "str_i64_new"),
                            args: Vec::new(),
                        });
                        for (k, v) in entries {
                            let key_v = match k {
                                RuntimeMapKeyData::ShortStr(key) | RuntimeMapKeyData::String(key) => {
                                    materialize_key(ssa, insts, globals, key)
                                }
                                _ => unreachable!("filtered to string keys above"),
                            };
                            let ConstRuntimeValueData::Bool(b) = v else {
                                unreachable!("filtered to bool values above");
                            };
                            let val_v = ssa.new_val();
                            insts.push(Inst::Const {
                                dst: val_v,
                                value: Const::I64(i64::from(*b)),
                            });
                            insts.push(Inst::Call {
                                dst: None,
                                callee: AbiRef::new("map_h", "str_i64_set"),
                                args: vec![handle, key_v, val_v],
                            });
                        }
                        ssa.write(instr.a(), block, (handle, Ty::MapStrBool));
                        return Ok(());
                    }
                    let all_int_vals = entries.iter().all(|(_, v)| matches!(v, ConstRuntimeValueData::Int(_)));
                    let all_f64_vals = entries
                        .iter()
                        .all(|(_, v)| matches!(v, ConstRuntimeValueData::Float(_)));
                    let all_int_keys = entries.iter().all(|(k, _)| matches!(k, RuntimeMapKeyData::Int(_)));
                    if !(all_int_vals || all_f64_vals) || !(all_str_keys || all_int_keys) {
                        // Mixed scalar values with string keys: a boxed-dynamic
                        // map (`Map<str, LkDyn>`, plan M4.2). Display stays out
                        // of the subset (hash order); nested containers and
                        // non-string keys still fall back.
                        let scalar_vals = entries.iter().all(|(_, v)| {
                            matches!(
                                v,
                                ConstRuntimeValueData::Nil
                                    | ConstRuntimeValueData::Bool(_)
                                    | ConstRuntimeValueData::Int(_)
                                    | ConstRuntimeValueData::Float(_)
                                    | ConstRuntimeValueData::ShortStr(_)
                            )
                        });
                        if !(all_str_keys && scalar_vals && !entries.is_empty()) {
                            return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                        }
                        let handle = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(handle),
                            callee: AbiRef::new("map_h", "str_dyn_new"),
                            args: Vec::new(),
                        });
                        for (k, v) in entries {
                            let key_v = match k {
                                RuntimeMapKeyData::ShortStr(key) | RuntimeMapKeyData::String(key) => {
                                    materialize_key(ssa, insts, globals, key)
                                }
                                _ => unreachable!("filtered to string keys above"),
                            };
                            let boxed = box_const_scalar(ssa, insts, globals, v);
                            insts.push(Inst::Call {
                                dst: None,
                                callee: AbiRef::new("map_h", "str_dyn_set"),
                                args: vec![handle, key_v, boxed],
                            });
                        }
                        ssa.write(instr.a(), block, (handle, Ty::MapStrDyn));
                        return Ok(());
                    }
                    // An empty `{}` is ambiguous: a lookahead types the key (a wrong
                    // guess only costs a fallback), and the value defaults to `i64`.
                    let int_keyed = if entries.is_empty() {
                        empty_map_is_int_keyed(&func.code, pc, instr.a())
                    } else {
                        all_int_keys
                    };
                    let f64_valued = !entries.is_empty() && all_f64_vals;
                    let (new_fn, set_fn, map_ty) = match (int_keyed, f64_valued) {
                        (false, false) => ("str_i64_new", "str_i64_set", Ty::MapStrI64),
                        (true, false) => ("i64_i64_new", "i64_i64_set", Ty::MapI64I64),
                        (false, true) => ("str_f64_new", "str_f64_set", Ty::MapStrF64),
                        (true, true) => ("i64_f64_new", "i64_f64_set", Ty::MapI64F64),
                    };
                    let handle = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(handle),
                        callee: AbiRef::new("map_h", new_fn),
                        args: Vec::new(),
                    });
                    for (k, v) in entries {
                        let key_v = match k {
                            RuntimeMapKeyData::ShortStr(key) | RuntimeMapKeyData::String(key) => {
                                materialize_key(ssa, insts, globals, key)
                            }
                            RuntimeMapKeyData::Int(ik) => {
                                let kv = ssa.new_val();
                                insts.push(Inst::Const {
                                    dst: kv,
                                    value: Const::I64(*ik),
                                });
                                kv
                            }
                            _ => unreachable!("filtered to string/int keys above"),
                        };
                        let value = match v {
                            ConstRuntimeValueData::Int(iv) => Const::I64(*iv),
                            ConstRuntimeValueData::Float(fv) => Const::F64(*fv),
                            _ => unreachable!("filtered to int/float values above"),
                        };
                        let val_v = ssa.new_val();
                        insts.push(Inst::Const { dst: val_v, value });
                        insts.push(Inst::Call {
                            dst: None,
                            callee: AbiRef::new("map_h", set_fn),
                            args: vec![handle, key_v, val_v],
                        });
                    }
                    ssa.write(instr.a(), block, (handle, map_ty));
                }
                ConstHeapValueData::LongString(s) => {
                    // A string literal too long for the inline `ShortStr` encoding:
                    // same lowering as `LoadString` (an interned C-string global).
                    let gid = intern_global(globals, s);
                    let dst = ssa.new_val();
                    insts.push(Inst::Const {
                        dst,
                        value: Const::Str(GlobalId(gid)),
                    });
                    ssa.const_strs.insert(dst, s.clone());
                    ssa.write(instr.a(), block, (dst, Ty::Str));
                }
                ConstHeapValueData::UpvalCell(initial) => {
                    // The compiler's shared mutable box for a captured local.
                    // The cell never materializes: its content lives in a
                    // virtual SSA slot (`reg_count + cid`) under the same
                    // Braun construction as registers, so cross-block state
                    // (mutation in a branch, reads after a merge, loop-carried
                    // updates) gets phis. Cells start nil (any pre-store read
                    // is a `Nil` value, exactly the VM's fresh-cell content);
                    // re-executing this site (a loop-created cell) re-
                    // initializes the slot, matching the VM's fresh cell.
                    if !matches!(initial.as_ref(), ConstRuntimeValueData::Nil) {
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                    }
                    let cid = ssa.next_cell;
                    ssa.next_cell += 1;
                    let nil = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: nil,
                        value: Const::Nil,
                    });
                    let slot = ssa.cell_slot(cid);
                    ssa.write_slot(slot, block, (nil, Ty::Nil));
                    ssa.builtin_regs.insert((block, instr.a()), GlobalRef::Cell(cid));
                }
            }
        }
        Opcode::Len => {
            // `a` = dst, `b` = container register; the length is always a plain `i64`,
            // regardless of element type (lists) or key/value type (maps).
            let (handle, ty) = ssa.read(instr.b(), block, pc)?;
            let (module, len_fn) = match ty {
                // Strings count Unicode scalar values (the VM's char length).
                Ty::Str => ("str", "char_len"),
                Ty::ListI64 => ("list_h", "i64_len"),
                Ty::ListF64 => ("list_h", "f64_len"),
                Ty::ListStr => ("list_h", "str_len"),
                Ty::MapStrI64 => ("map_h", "str_i64_len"),
                Ty::MapI64I64 => ("map_h", "i64_i64_len"),
                Ty::MapStrF64 => ("map_h", "str_f64_len"),
                Ty::MapI64F64 => ("map_h", "i64_f64_len"),
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new(module, len_fn),
                args: vec![handle],
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
        }
        Opcode::SliceFrom => {
            // `a` = dst, `b` = target (list) register, `c` = start register. Only
            // typed lists lower natively — the runtime returns a fresh handle
            // with the elements from `start` on (negative `start` aborts, like
            // the VM). String slicing and other element types fall back for now.
            let (handle, ty) = ssa.read(instr.b(), block, pc)?;
            let slice_fn = match ty {
                Ty::ListI64 => "i64_slice_from",
                Ty::ListF64 => "f64_slice_from",
                Ty::ListStr => "str_slice_from",
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let start = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", slice_fn),
                args: vec![handle, start],
            });
            ssa.write(instr.a(), block, (dst, ty));
        }
        Opcode::StringSplit => {
            // `a` = dst (List<str>), `b` = target string, `c` = separator string.
            // The runtime uses Rust `str::split`, so the result matches the VM's
            // `string_split` exactly.
            let target = ssa.read_typed(instr.b(), block, Ty::Str, pc)?;
            let sep = ssa.read_typed(instr.c(), block, Ty::Str, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "split"),
                args: vec![target, sep],
            });
            ssa.write(instr.a(), block, (dst, Ty::ListStr));
        }
        Opcode::ListPush => {
            // `a` = list register (mutated in place), `b` = value register. The list
            // handle is a reference (matching the VM), so the push is visible through
            // aliases; no new SSA value is produced for the list.
            let (handle, list_ty) = ssa.read(instr.a(), block, pc)?;
            match list_ty {
                Ty::ListI64 => {
                    let value = ssa.read_typed(instr.b(), block, Ty::I64, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "i64_push"),
                        args: vec![handle, value],
                    });
                }
                Ty::ListF64 => {
                    let (bv, bty) = ssa.read(instr.b(), block, pc)?;
                    if !matches!(bty, Ty::I64 | Ty::F64) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let value = coerce_to_f64(ssa, insts, bv, bty);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "f64_push"),
                        args: vec![handle, value],
                    });
                }
                Ty::ListStr => {
                    // Stored strings are arena-owned (interned constants or
                    // register-visible arena strings), alive until exit, so the
                    // pointer push involves no ownership transfer.
                    let value = ssa.read_typed(instr.b(), block, Ty::Str, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "str_push"),
                        args: vec![handle, value],
                    });
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
            // Keep the known length in sync so subsequent constant-index bounds
            // checks stay accurate (only meaningful for a still-tracked handle).
            if let Some(len) = ssa.list_len.get_mut(&handle) {
                *len += 1;
            }
        }
        // `GetList` is the list-typed index; `GetIndex` is the generic index the
        // compiler emits when it hasn't proven the container is a list (e.g. inside a
        // `for x in xs` loop body). For a list operand both have identical semantics,
        // so they share this arm; a non-list operand rejects (falls back).
        Opcode::GetList | Opcode::GetIndex => {
            // A constant-name member read on a module object resolves to a
            // module function ref (`os.clock` → `GetIndex` on the module with a
            // constant string key) — or, for constant members (`math.pi`), to
            // the literal value itself.
            if let Some(GlobalRef::Module(module)) = ssa.builtin_regs.get(&(block, instr.b())).cloned() {
                let name = {
                    let key = ssa.read(instr.c(), block, pc).ok().map(|(v, _)| v);
                    key.and_then(|v| ssa.const_strs.get(&v).cloned())
                        .or_else(|| ssa.reg_const_str(instr.c(), block))
                };
                let Some(name) = name else {
                    return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                };
                if let Some((value, ty)) = module_const(&module, &name) {
                    let dst = ssa.new_val();
                    insts.push(Inst::Const { dst, value });
                    ssa.write(instr.a(), block, (dst, ty));
                    return Ok(());
                }
                ssa.builtin_regs
                    .insert((block, instr.a()), GlobalRef::ModuleFn(module, name));
                return Ok(());
            }
            // `a` = dst, `b` = container register, `c` = key register.
            let (handle, list_ty) = ssa.read(instr.b(), block, pc)?;
            // String-keyed map reads take a `Str` key (dynamic template keys
            // included); a missing key is the `Maybe` nil model.
            if matches!(list_ty, Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool) {
                let key = ssa.read_typed(instr.c(), block, Ty::Str, pc)?;
                let dst = ssa.new_val();
                let maybe_ty = match list_ty {
                    Ty::MapStrF64 => {
                        insts.push(Inst::MapGetMaybeStrF64 { dst, handle, key });
                        Ty::MaybeF64
                    }
                    Ty::MapStrBool => {
                        insts.push(Inst::MapGetMaybe { dst, handle, key });
                        Ty::MaybeBool
                    }
                    _ => {
                        insts.push(Inst::MapGetMaybe { dst, handle, key });
                        Ty::MaybeI64
                    }
                };
                ssa.write(instr.a(), block, (dst, maybe_ty));
                return Ok(());
            }
            // Lists / int-keyed maps index with an `I64` (a `Maybe` index —
            // `xs[ys[j]]` — unwraps first).
            let index_val = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
            // Fast path: a **provably in-range** access (constant list of known
            // length indexed by a constant in `[0, len)`) is a clean scalar `at`.
            let const_in_range = match (ssa.list_len.get(&handle), ssa.const_int.get(&index_val)) {
                (Some(&len), Some(&idx)) if idx >= 0 && idx < len => Some(idx),
                _ => None,
            };
            if let Some(idx) = const_in_range {
                let (at_fn, elem_ty) = match list_ty {
                    Ty::ListI64 => ("i64_at", Ty::I64),
                    Ty::ListF64 => ("f64_at", Ty::F64),
                    Ty::ListStr => ("str_at", Ty::Str),
                    // Mixed list: the element is a boxed Dyn either way
                    // (`dyn_at` handles negative/OOB as a Nil-tag Dyn).
                    Ty::ListDyn => ("dyn_at", Ty::Dyn),
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                let idx_v = ssa.new_val();
                insts.push(Inst::Const {
                    dst: idx_v,
                    value: Const::I64(idx),
                });
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("list_h", at_fn),
                    args: vec![handle, idx_v],
                });
                ssa.write(instr.a(), block, (dst, elem_ty));
            } else {
                // Dynamic / not-provably-in-range: the result is `Maybe<Int>` (VM:
                // out-of-range or negative → nil). Model it explicitly as
                // [`Ty::MaybeI64`]; its only supported consumer is a function return
                // (which prints the element or `nil`, matching the VM byte-for-byte).
                // A scalar consumer unwraps this via `read_scalar` (present-assert,
                // matching the VM's halt on `nil` arithmetic); a `return` keeps it
                // and prints `nil`. Either way there is no eager-abort shortcut that
                // would diverge from `return xs[oob]` printing `nil`.
                match list_ty {
                    // Mixed list: no Maybe carrier needed — the Dyn's Nil tag
                    // *is* the absent case (`dyn_at` maps OOB/negative-beyond
                    // to Nil, matching the VM's nil-on-out-of-range).
                    Ty::ListDyn => {
                        let dst = ssa.new_val();
                        insts.push(Inst::Call {
                            dst: Some(dst),
                            callee: AbiRef::new("list_h", "dyn_at"),
                            args: vec![handle, index_val],
                        });
                        ssa.write(instr.a(), block, (dst, Ty::Dyn));
                    }
                    Ty::ListI64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::ListGetMaybe {
                            dst,
                            handle,
                            index: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeI64));
                    }
                    Ty::ListF64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::ListGetMaybeF64 {
                            dst,
                            handle,
                            index: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeF64));
                    }
                    Ty::ListStr => {
                        let dst = ssa.new_val();
                        insts.push(Inst::ListGetMaybeStr {
                            dst,
                            handle,
                            index: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeStr));
                    }
                    // Int-keyed map lookup (`m[k]`): the key is the read index; a
                    // missing key is `nil`, i.e. the same `Maybe` model.
                    Ty::MapI64I64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::MapGetMaybeI64Key {
                            dst,
                            handle,
                            key: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeI64));
                    }
                    Ty::MapI64F64 => {
                        let dst = ssa.new_val();
                        insts.push(Inst::MapGetMaybeI64F64 {
                            dst,
                            handle,
                            key: index_val,
                        });
                        ssa.write(instr.a(), block, (dst, Ty::MaybeF64));
                    }
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                }
            }
        }
        Opcode::SetIndex => {
            // `a` = container register, `b` = index/key register, `c` = value register.
            // For a **list**, the store is bounds-checked in the runtime helper (aborts
            // on an out-of-range/negative index — the VM's fatal store error, a halt).
            // For a **map**, the store always inserts-or-updates. An unsupported
            // container/key/value combination rejects (falls back).
            let (handle, list_ty) = ssa.read(instr.a(), block, pc)?;
            // String-keyed map stores take a `Str` key (dynamic template keys
            // included); the map ABI copies the key.
            if matches!(list_ty, Ty::MapStrI64 | Ty::MapStrF64) {
                let key = ssa.read_typed(instr.b(), block, Ty::Str, pc)?;
                let (cv, cty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                let (set_fn, value) = match (list_ty, cty) {
                    (Ty::MapStrI64, Ty::I64) => ("str_i64_set", cv),
                    (Ty::MapStrF64, Ty::F64) => ("str_f64_set", cv),
                    (Ty::MapStrF64, Ty::I64) => ("str_f64_set", coerce_to_f64(ssa, insts, cv, cty)),
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", set_fn),
                    args: vec![handle, key, value],
                });
                return Ok(());
            }
            let index = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
            match list_ty {
                Ty::ListI64 => {
                    let value = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "i64_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::ListF64 => {
                    let (cv, cty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                    if !matches!(cty, Ty::I64 | Ty::F64) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let value = coerce_to_f64(ssa, insts, cv, cty);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("list_h", "f64_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::MapI64I64 => {
                    let value = read_typed_scalar(ssa, insts, instr.c(), block, Ty::I64, pc)?;
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("map_h", "i64_i64_set"),
                        args: vec![handle, index, value],
                    });
                }
                Ty::MapI64F64 => {
                    let (cv, cty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
                    if !matches!(cty, Ty::I64 | Ty::F64) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    let value = coerce_to_f64(ssa, insts, cv, cty);
                    insts.push(Inst::Call {
                        dst: None,
                        callee: AbiRef::new("map_h", "i64_f64_set"),
                        args: vec![handle, index, value],
                    });
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
        }
        Opcode::GetFieldK => {
            // `a` = dst, `b` = map register, `c` = key string-constant index. A
            // missing key is `nil` → the `Maybe` model (i64- or f64-valued map).
            let (handle, map_ty) = ssa.read(instr.b(), block, pc)?;
            let key = func
                .consts
                .strings
                .get(instr.c() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let key_v = materialize_key(ssa, insts, globals, key);
            let dst = ssa.new_val();
            let result_ty = match map_ty {
                Ty::MapStrBool => {
                    insts.push(Inst::MapGetMaybe {
                        dst,
                        handle,
                        key: key_v,
                    });
                    Ty::MaybeBool
                }
                Ty::MapStrI64 => {
                    insts.push(Inst::MapGetMaybe {
                        dst,
                        handle,
                        key: key_v,
                    });
                    Ty::MaybeI64
                }
                Ty::MapStrF64 => {
                    insts.push(Inst::MapGetMaybeStrF64 {
                        dst,
                        handle,
                        key: key_v,
                    });
                    Ty::MaybeF64
                }
                // Mixed-value map: the Dyn carrier's Nil tag *is* the
                // missing-key case — no Maybe wrapper needed.
                Ty::MapStrDyn => {
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("map_h", "str_dyn_get"),
                        args: vec![handle, key_v],
                    });
                    Ty::Dyn
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            ssa.write(instr.a(), block, (dst, result_ty));
        }
        Opcode::SetFieldK => {
            // `a` = map register, `b` = value register, `c` = key string-constant
            // index. A store always inserts-or-updates (never an error).
            let (handle, map_ty) = ssa.read(instr.a(), block, pc)?;
            let key = func
                .consts
                .strings
                .get(instr.c() as usize)
                .ok_or(Unsupported::BadConst { pc })?;
            let key_v = materialize_key(ssa, insts, globals, key);
            let (set_fn, value) = match map_ty {
                Ty::MapStrI64 => (
                    "str_i64_set",
                    read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?,
                ),
                Ty::MapStrF64 => {
                    let (bv, bty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
                    if !matches!(bty, Ty::I64 | Ty::F64) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    ("str_f64_set", coerce_to_f64(ssa, insts, bv, bty))
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("map_h", set_fn),
                args: vec![handle, key_v, value],
            });
        }
        Opcode::Contains => {
            // `a` = dst (bool), `b` = needle, `c` = haystack. List and string-keyed
            // map haystacks are lowered; other haystacks fall back.
            let (handle, list_ty) = ssa.read(instr.c(), block, pc)?;
            // `key in map` tests key membership (VM `map_contains`): read the
            // map's `Maybe` for the key and take its present bit — no value
            // materialization needed. Mirrors the map `GetIndex` path.
            if matches!(list_ty, Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool) {
                let key = ssa.read_typed(instr.b(), block, Ty::Str, pc)?;
                let maybe = ssa.new_val();
                let maybe_ty = match list_ty {
                    Ty::MapStrF64 => {
                        insts.push(Inst::MapGetMaybeStrF64 {
                            dst: maybe,
                            handle,
                            key,
                        });
                        Ty::MaybeF64
                    }
                    Ty::MapStrBool => {
                        insts.push(Inst::MapGetMaybe {
                            dst: maybe,
                            handle,
                            key,
                        });
                        Ty::MaybeBool
                    }
                    _ => {
                        insts.push(Inst::MapGetMaybe {
                            dst: maybe,
                            handle,
                            key,
                        });
                        Ty::MaybeI64
                    }
                };
                let dst = ssa.new_val();
                insts.push(Inst::MaybePresent {
                    dst,
                    src: maybe,
                    maybe_ty,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            // Int-keyed maps: same present-bit test with an `I64` key.
            if matches!(list_ty, Ty::MapI64I64 | Ty::MapI64F64) {
                let key = read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?;
                let maybe = ssa.new_val();
                let maybe_ty = if list_ty == Ty::MapI64F64 {
                    insts.push(Inst::MapGetMaybeI64F64 {
                        dst: maybe,
                        handle,
                        key,
                    });
                    Ty::MaybeF64
                } else {
                    insts.push(Inst::MapGetMaybeI64Key {
                        dst: maybe,
                        handle,
                        key,
                    });
                    Ty::MaybeI64
                };
                let dst = ssa.new_val();
                insts.push(Inst::MaybePresent {
                    dst,
                    src: maybe,
                    maybe_ty,
                });
                ssa.write(instr.a(), block, (dst, Ty::Bool));
                return Ok(());
            }
            let (fn_name, needle) = match list_ty {
                Ty::ListI64 => (
                    "i64_contains",
                    read_typed_scalar(ssa, insts, instr.b(), block, Ty::I64, pc)?,
                ),
                Ty::ListF64 => {
                    let (nv, nty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
                    if !matches!(nty, Ty::I64 | Ty::F64) {
                        return Err(Unsupported::TypeMismatch { pc });
                    }
                    ("f64_contains", coerce_to_f64(ssa, insts, nv, nty))
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let raw = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(raw),
                callee: AbiRef::new("list_h", fn_name),
                args: vec![handle, needle],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Cmp {
                dst,
                op: CmpOp::Ne,
                float: false,
                lhs: raw,
                rhs: zero,
            });
            ssa.write(instr.a(), block, (dst, Ty::Bool));
        }
        Opcode::Raise => {
            // `bx` = the raised message string constant. A `Raise` with no
            // enclosing handler aborts, exactly like `panic`. The native path
            // only reaches here when the module has no try/catch at all: `TryBegin`
            // is itself unsupported and forces the Tier 0 fallback, so any module
            // that lowers natively cannot catch a raise — every lowered `Raise` is
            // uncaught. (The differential harness already treats VM exit-1 and a
            // native SIGABRT as matching failures, as it does for `assert`/`panic`.)
            let message = func
                .consts
                .strings
                .get(instr.bx() as usize)
                .ok_or(Unsupported::BadConst { pc })?
                .clone();
            let msg = materialize_key(ssa, insts, globals, &message);
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "panic"),
                args: vec![msg],
            });
        }
        Opcode::MapRest => {
            // `a` = dst, `b` = base (source map), `c` = key_count. The result is
            // the map with the `key_count` string keys in registers
            // base+1..=base+key_count removed — one `without` call chained per
            // key (matching the VM's `map_rest`). Only string-keyed maps lower.
            let base = instr.b();
            let key_count = instr.c();
            let (map_handle, map_ty) = ssa.read(base, block, pc)?;
            let without_fn = match map_ty {
                Ty::MapStrI64 | Ty::MapStrBool => "str_i64_without",
                Ty::MapStrF64 => "str_f64_without",
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            let mut current = map_handle;
            for offset in 0..key_count {
                let key_reg = base
                    .checked_add(1)
                    .and_then(|r| r.checked_add(offset))
                    .ok_or(Unsupported::TypeMismatch { pc })?;
                let key = ssa.read_typed(key_reg, block, Ty::Str, pc)?;
                let next = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(next),
                    callee: AbiRef::new("map_h", without_fn),
                    args: vec![current, key],
                });
                current = next;
            }
            ssa.write(instr.a(), block, (current, map_ty));
        }
        // Control-flow opcodes are terminators, normally handled outside lower_inst.
        // Reaching here means a branch targeted the middle of a fused pair or an
        // otherwise malformed shape — reject cleanly (fall back) rather than panic.
        op => return Err(Unsupported::Opcode { pc, op }),
    }
    Ok(())
}

fn int_bin_op(op: Opcode) -> IntBinOp {
    match op {
        Opcode::AddInt => IntBinOp::Add,
        Opcode::SubInt => IntBinOp::Sub,
        Opcode::MulInt => IntBinOp::Mul,
        Opcode::DivInt => IntBinOp::Div,
        Opcode::ModInt => IntBinOp::Mod,
        _ => unreachable!("integer arithmetic opcode"),
    }
}

fn imm_int_bin_op(op: Opcode) -> IntBinOp {
    match op {
        Opcode::AddIntI => IntBinOp::Add,
        Opcode::MulIntI => IntBinOp::Mul,
        Opcode::ModIntI => IntBinOp::Mod,
        _ => unreachable!("immediate integer opcode"),
    }
}

/// Emits a widening cast if needed so `v` is an `f64` (no-op if already `f64`).
fn coerce_to_f64(ssa: &mut Ssa, insts: &mut Vec<Inst>, v: ValueId, ty: Ty) -> ValueId {
    if ty == Ty::F64 {
        return v;
    }
    let f = ssa.new_val();
    insts.push(Inst::IntToFloat { dst: f, src: v });
    f
}

/// Reads a register for a **scalar** (arithmetic/comparison/call/store) context,
/// narrowing a [`Ty::MaybeI64`] to `I64` via a present-asserting unwrap
/// ([`Inst::UnwrapMaybeI64`], which aborts if absent — matching the VM's halt on
/// `nil` arithmetic). Every other type passes through unchanged. This is the
/// scalar-consumer counterpart of a bare `ssa.read` (which a `return` uses instead,
/// to keep the `Maybe` and print `nil`).
fn read_scalar(ssa: &mut Ssa, insts: &mut Vec<Inst>, reg: u8, block: usize, pc: usize) -> Result<Reg, Unsupported> {
    let (v, ty) = ssa.read(reg, block, pc)?;
    match ty {
        Ty::MaybeI64 => {
            let dst = ssa.new_val();
            insts.push(Inst::UnwrapMaybeI64 { dst, src: v });
            Ok((dst, Ty::I64))
        }
        Ty::MaybeF64 => {
            let dst = ssa.new_val();
            insts.push(Inst::UnwrapMaybeF64 { dst, src: v });
            Ok((dst, Ty::F64))
        }
        Ty::MaybeStr => {
            let dst = ssa.new_val();
            insts.push(Inst::UnwrapMaybeStr { dst, src: v });
            Ok((dst, Ty::Str))
        }
        Ty::MaybeBool => {
            // Same abort-on-absent narrowing as MaybeI64, then re-typed to Bool.
            let wide = ssa.new_val();
            insts.push(Inst::UnwrapMaybeI64 { dst: wide, src: v });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Cmp {
                dst,
                op: CmpOp::Ne,
                float: false,
                lhs: wide,
                rhs: zero,
            });
            Ok((dst, Ty::Bool))
        }
        _ => Ok((v, ty)),
    }
}

/// [`read_scalar`] that also requires a specific type (the unwrap-aware counterpart
/// of `Ssa::read_typed`).
fn read_typed_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    reg: u8,
    block: usize,
    want: Ty,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let (v, ty) = read_scalar(ssa, insts, reg, block, pc)?;
    if ty == want {
        Ok(v)
    } else {
        Err(Unsupported::TypeMismatch { pc })
    }
}

/// Converts a scalar to its display `Str` (the VM's `ToString`/interpolation
/// conversion): a `Str` passes through; `I64`/`F64`/`Bool` go through the display
/// helpers (which use the exact Rust formatting the VM uses, so output matches
/// byte-for-byte). Containers/`Maybe` reject (fall back).
/// Display-converts a value to a `Str`. The returned flag is `true` when the
/// string is a *fresh* runtime allocation created here (a `*_to_str` result) whose
/// only consumer is the caller — such temporaries may be freed once consumed
/// (`free_owned_str`), realizing the RFC §3.4 ownership model for known-dead
/// intermediates. A pre-existing `Str` (interned global or register value) is
/// returned as-is with `false`.
/// `containers` mirrors the VM's two display paths: the stdlib
/// `runtime_display` (print/println/panic/assert messages) renders containers,
/// while the executor's `runtime_value_display_string` (`ToString`, template
/// interpolation, `+` concatenation) is scalar-only and errors loudly on a
/// container — so container display must reject in those contexts.
fn to_display_str(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    v: ValueId,
    ty: Ty,
    containers: bool,
    pc: usize,
) -> Result<(ValueId, bool), Unsupported> {
    match ty {
        Ty::Str => Ok((v, false)),
        // A `Maybe` displays its value when present and `nil` when absent
        // (matching the VM's display of a missing-key read). The value-side
        // conversion runs unconditionally (its result is arena-owned and
        // simply unused on the absent path), then a select picks the text.
        Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
            let raw = ssa.new_val();
            insts.push(Inst::MaybeValue {
                dst: raw,
                src: v,
                maybe_ty: ty,
            });
            let scalar_ty = match ty {
                Ty::MaybeI64 | Ty::MaybeBool => Ty::I64,
                Ty::MaybeF64 => Ty::F64,
                _ => Ty::Str,
            };
            // Bool display goes through from_bool, not the i64 decimal text.
            let raw = if ty == Ty::MaybeBool {
                let zero = ssa.new_val();
                insts.push(Inst::Const {
                    dst: zero,
                    value: Const::I64(0),
                });
                let b = ssa.new_val();
                insts.push(Inst::Cmp {
                    dst: b,
                    op: CmpOp::Ne,
                    float: false,
                    lhs: raw,
                    rhs: zero,
                });
                b
            } else {
                raw
            };
            let scalar_ty = if ty == Ty::MaybeBool { Ty::Bool } else { scalar_ty };
            let (value_str, _) = to_display_str(ssa, insts, globals, raw, scalar_ty, false, pc)?;
            let present = ssa.new_val();
            insts.push(Inst::MaybePresent {
                dst: present,
                src: v,
                maybe_ty: ty,
            });
            let nil_gid = intern_global(globals, "nil");
            let nil_str = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil_str,
                value: Const::Str(GlobalId(nil_gid)),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Select {
                dst,
                cond: present,
                then_v: value_str,
                else_v: nil_str,
                ty: Ty::Str,
            });
            // Not marked fresh: the value-side temporary stays arena-owned
            // (freeing it eagerly would dangle when the select picked it).
            Ok((dst, false))
        }
        Ty::I64 => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "from_i64"),
                args: vec![v],
            });
            Ok((dst, true))
        }
        Ty::F64 => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "from_f64"),
                args: vec![v],
            });
            Ok((dst, true))
        }
        Ty::Bool => {
            let wide = ssa.new_val();
            insts.push(Inst::ZextBool { dst: wide, src: v });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "from_bool"),
                args: vec![wide],
            });
            Ok((dst, true))
        }
        // List display (`[1,2,3]` / `["a","b c"]`) renders inside lkrt with
        // the VM's exact separators/quoting. Map display stays out of the
        // subset: its order is the underlying hash iteration order, which is
        // not portable across the two runtimes (see docs/semantics.md).
        Ty::ListI64 | Ty::ListF64 | Ty::ListStr | Ty::ListDyn => {
            if !containers {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let display_fn = match ty {
                Ty::ListI64 => "i64_display",
                Ty::ListF64 => "f64_display",
                Ty::ListDyn => "dyn_display",
                _ => "str_display",
            };
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", display_fn),
                args: vec![v],
            });
            Ok((dst, true))
        }
        // A boxed Dyn from a mixed-list read: at runtime it is a scalar in
        // D2 (nested containers never box — see LoadHeapConst's scalar_only
        // guard), so the bare display mode is exact for both display paths.
        Ty::Dyn => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("dyn", "display"),
                args: vec![v],
            });
            Ok((dst, true))
        }
        _ => Err(Unsupported::TypeMismatch { pc }),
    }
}

/// Emits `acc ++ display(v)`. An `I64` operand fuses into a single
/// `str.concat_i64` call (no intermediate suffix string); every other display
/// type goes through [`to_display_str`] + `str.concat`, eagerly freeing the
/// fresh display temporary.
#[allow(clippy::too_many_arguments)]
fn concat_display(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    acc: ValueId,
    v: ValueId,
    ty: Ty,
    containers: bool,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    if ty == Ty::I64 {
        let dst = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(dst),
            callee: AbiRef::new("str", "concat_i64"),
            args: vec![acc, v],
        });
        return Ok(dst);
    }
    let (s, fresh) = to_display_str(ssa, insts, globals, v, ty, containers, pc)?;
    let dst = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(dst),
        callee: AbiRef::new("str", "concat"),
        args: vec![acc, s],
    });
    if fresh {
        free_owned_str(insts, s);
    }
    Ok(dst)
}

/// Frees a fresh, lower-created string temporary that has been fully consumed.
/// Sound only for values invisible to user code (display temporaries and
/// intermediate concat accumulators).
fn free_owned_str(insts: &mut Vec<Inst>, v: ValueId) {
    insts.push(Inst::Call {
        dst: None,
        callee: AbiRef::new("lkrt", "string_free"),
        args: vec![v],
    });
}

/// Lowers a call to a recognized runtime builtin (`println` / `print` /
/// `assert`). The builtin's nil return is written to the call-window base
/// register, matching the VM's return-value placement.
#[allow(clippy::too_many_arguments)]
fn lower_builtin_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    builtin: Builtin,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    match builtin {
        Builtin::Println | Builtin::Print => {
            let parts = print_parts(ssa, base, argc, block, pc)?;
            emit_print(ssa, insts, globals, parts, builtin == Builtin::Println, pc)?;
        }
        Builtin::CallMethod => {
            // Dispatched by the caller before reaching here.
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        Builtin::Panic => {
            // `panic(args…)`: the message is the space-joined display of the
            // arguments (`join_runtime_display`), or the literal `panic` with
            // no arguments; always fatal (the VM's loud panic halt).
            let msg = if argc == 0 {
                materialize_key(ssa, insts, globals, "panic")
            } else {
                let (v0, ty0) = ssa.read(base.wrapping_add(1), block, pc)?;
                let (mut acc, mut acc_fresh) = to_display_str(ssa, insts, globals, v0, ty0, true, pc)?;
                for i in 1..argc {
                    let sep = materialize_key(ssa, insts, globals, " ");
                    let with_sep = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(with_sep),
                        callee: AbiRef::new("str", "concat"),
                        args: vec![acc, sep],
                    });
                    if acc_fresh {
                        free_owned_str(insts, acc);
                    }
                    let (v, ty) = ssa.read(base.wrapping_add(1 + i as u8), block, pc)?;
                    acc = concat_display(ssa, insts, globals, with_sep, v, ty, true, pc)?;
                    free_owned_str(insts, with_sep);
                    acc_fresh = true;
                }
                acc
            };
            // The call aborts and never returns; the message is intentionally
            // not freed.
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "panic"),
                args: vec![msg],
            });
        }
        Builtin::AssertEq | Builtin::AssertNe => {
            // `assert_eq(a, b [, extra])` / `assert_ne`: scalar equality with
            // the VM's `runtime_values_equal` semantics (same-type scalars,
            // Int/Float coercion, byte-equal strings). The failure message is
            // built eagerly (dead on the success path) so no extra control
            // flow is needed.
            if !(2..=3).contains(&argc) {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let negated = builtin == Builtin::AssertNe;
            let (lv, lty) = ssa.read(base.wrapping_add(1), block, pc)?;
            let (rv, rty) = ssa.read(base.wrapping_add(2), block, pc)?;
            let op = if negated { CmpOp::Ne } else { CmpOp::Eq };
            let ok = match (lty, rty) {
                (Ty::I64, Ty::I64) | (Ty::Bool, Ty::Bool) => {
                    let dst = ssa.new_val();
                    insts.push(Inst::Cmp {
                        dst,
                        op,
                        float: false,
                        lhs: lv,
                        rhs: rv,
                    });
                    dst
                }
                (Ty::F64, Ty::F64) | (Ty::I64, Ty::F64) | (Ty::F64, Ty::I64) => {
                    let widen = |ssa: &mut Ssa, insts: &mut Vec<Inst>, v: ValueId, ty: Ty| {
                        if ty == Ty::I64 {
                            let f = ssa.new_val();
                            insts.push(Inst::IntToFloat { dst: f, src: v });
                            f
                        } else {
                            v
                        }
                    };
                    let lf = widen(ssa, insts, lv, lty);
                    let rf = widen(ssa, insts, rv, rty);
                    let dst = ssa.new_val();
                    insts.push(Inst::Cmp {
                        dst,
                        op,
                        float: true,
                        lhs: lf,
                        rhs: rf,
                    });
                    dst
                }
                (Ty::Str, Ty::Str) => {
                    let cmp = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(cmp),
                        callee: AbiRef::new("str", "cmp"),
                        args: vec![lv, rv],
                    });
                    let zero = ssa.new_val();
                    insts.push(Inst::Const {
                        dst: zero,
                        value: Const::I64(0),
                    });
                    let dst = ssa.new_val();
                    insts.push(Inst::Cmp {
                        dst,
                        op,
                        float: false,
                        lhs: cmp,
                        rhs: zero,
                    });
                    dst
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            };
            // `(msg, fresh)`: a bare const message must not be freed.
            let (msg, msg_fresh) = if negated {
                // "values should not be equal" — no operand displays.
                if argc == 3 {
                    let (ev, ety) = ssa.read(base.wrapping_add(3), block, pc)?;
                    let sep = materialize_key(ssa, insts, globals, "values should not be equal - ");
                    (concat_display(ssa, insts, globals, sep, ev, ety, true, pc)?, true)
                } else {
                    (
                        materialize_key(ssa, insts, globals, "values should not be equal"),
                        false,
                    )
                }
            } else {
                // "expected {b}, got {a}" (+ " - {extra}").
                let head = materialize_key(ssa, insts, globals, "expected ");
                let with_expected = concat_display(ssa, insts, globals, head, rv, rty, true, pc)?;
                let comma = materialize_key(ssa, insts, globals, ", got ");
                let joined = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(joined),
                    callee: AbiRef::new("str", "concat"),
                    args: vec![with_expected, comma],
                });
                free_owned_str(insts, with_expected);
                let full = concat_display(ssa, insts, globals, joined, lv, lty, true, pc)?;
                free_owned_str(insts, joined);
                if argc == 3 {
                    let (ev, ety) = ssa.read(base.wrapping_add(3), block, pc)?;
                    let dash = materialize_key(ssa, insts, globals, " - ");
                    let with_dash = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(with_dash),
                        callee: AbiRef::new("str", "concat"),
                        args: vec![full, dash],
                    });
                    free_owned_str(insts, full);
                    let all = concat_display(ssa, insts, globals, with_dash, ev, ety, true, pc)?;
                    free_owned_str(insts, with_dash);
                    (all, true)
                } else {
                    (full, true)
                }
            };
            let wide = ssa.new_val();
            insts.push(Inst::ZextBool { dst: wide, src: ok });
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("rt", "assert_msg"),
                args: vec![wide, msg],
            });
            if msg_fresh {
                free_owned_str(insts, msg);
            }
        }
        Builtin::Typeof => {
            // `typeof(x)` — the VM's type name from the statically proven MIR
            // type. Maybe carriers select between the scalar name and `Nil` at
            // runtime (a missing map key is `Nil` in the VM).
            if argc != 1 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let (v, ty) = ssa.read(base.wrapping_add(1), block, pc)?;
            let scalar_name = |ty: Ty| match ty {
                Ty::I64 => Some("Int"),
                Ty::F64 => Some("Float"),
                Ty::Bool => Some("Bool"),
                Ty::Str => Some("String"),
                Ty::Nil => Some("Nil"),
                _ => None,
            };
            let result = match ty {
                Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                    let value_name = match ty {
                        Ty::MaybeI64 => "Int",
                        Ty::MaybeF64 => "Float",
                        Ty::MaybeBool => "Bool",
                        _ => "String",
                    };
                    let present = ssa.new_val();
                    insts.push(Inst::MaybePresent {
                        dst: present,
                        src: v,
                        maybe_ty: ty,
                    });
                    let then_v = materialize_key(ssa, insts, globals, value_name);
                    let else_v = materialize_key(ssa, insts, globals, "Nil");
                    let dst = ssa.new_val();
                    insts.push(Inst::Select {
                        dst,
                        cond: present,
                        then_v,
                        else_v,
                        ty: Ty::Str,
                    });
                    dst
                }
                ty => match scalar_name(ty) {
                    Some(name) => materialize_key(ssa, insts, globals, name),
                    None => return Err(Unsupported::TypeMismatch { pc }),
                },
            };
            ssa.write(base, block, (result, Ty::Str));
            return Ok(());
        }
        Builtin::Assert => {
            // `assert(cond)` / `assert(cond, message)`: a false condition is a
            // fatal error, matching the VM's loud halt. The condition must be a
            // `Bool` (the VM's truthiness for other values is not modelled).
            if argc == 0 || argc > 2 {
                return Err(Unsupported::Opcode { pc, op: Opcode::Call });
            }
            let cond = ssa.read_typed(base.wrapping_add(1), block, Ty::Bool, pc)?;
            let wide = ssa.new_val();
            insts.push(Inst::ZextBool { dst: wide, src: cond });
            if argc == 1 {
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("rt", "assert"),
                    args: vec![wide],
                });
            } else {
                let (mv, mty) = ssa.read(base.wrapping_add(2), block, pc)?;
                let (msg, fresh) = to_display_str(ssa, insts, globals, mv, mty, true, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("rt", "assert_msg"),
                    args: vec![wide, msg],
                });
                // On failure the call aborts and never returns; on success the
                // display temporary is dead.
                if fresh {
                    free_owned_str(insts, msg);
                }
            }
        }
    }
    let nil = ssa.new_val();
    insts.push(Inst::Const {
        dst: nil,
        value: Const::Nil,
    });
    ssa.write(base, block, (nil, Ty::Nil));
    Ok(())
}

/// Lowers `__lk_call_method(receiver, name, args_list)` — the compiler's
/// generic method dispatch. The method name must be a compile-time constant
/// and the argument pack must be a lowering-tracked [`GlobalRef::ArgList`];
/// dispatch is per (receiver type, method name, argument types), each entry
/// mapped to a typed lkrt ABI call with VM-exact semantics.
fn lower_method_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    base: u8,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    let (receiver, receiver_ty) = ssa.read(base.wrapping_add(1), block, pc)?;
    let name_reg = base.wrapping_add(2);
    let name = {
        let name_v = ssa.read(name_reg, block, pc).ok().map(|(v, _)| v);
        name_v
            .and_then(|v| ssa.const_strs.get(&v).cloned())
            .or_else(|| ssa.reg_const_str(name_reg, block))
    };
    let Some(name) = name else {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    };
    let args = match ssa.builtin_regs.get(&(block, base.wrapping_add(3))) {
        Some(GlobalRef::ArgList(elems)) => elems.clone(),
        _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
    };
    let result = lower_method_dispatch(ssa, insts, globals, receiver, receiver_ty, &name, &args, block, pc)?;
    ssa.write(base, block, result);
    Ok(())
}

/// `CallMethodK` — the boxing-free method-call opcode: receiver at the window
/// base, args in the window, method name a string constant. Shares the
/// per-(receiver type, method) dispatch with the legacy
/// `__lk_call_method` shape.
#[allow(clippy::too_many_arguments)]
fn lower_method_call_k(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    func: &FunctionData,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    instr: &Instr,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    let base = instr.a();
    let name = func
        .consts
        .strings
        .get(instr.b() as usize)
        .ok_or(Unsupported::BadConst { pc })?
        .clone();
    let (receiver, receiver_ty) = ssa.read(base, block, pc)?;
    let argc = instr.c() as usize;
    // List HOF with a compiled zero-capture lambda callback (fn-pointer ABI):
    // handled before the generic argument reads, because the lambda register
    // carries a `GlobalRef::Lambda`, not an SSA value.
    if receiver_ty == Ty::ListI64
        && let Some(result) = lower_list_hof_k(ssa, insts, funcs, entry, sig, receiver, &name, base, argc, block, pc)?
    {
        ssa.write(base, block, result);
        return Ok(());
    }
    let mut args = Vec::with_capacity(argc);
    for i in 0..argc {
        args.push(ssa.read(base.wrapping_add(1).wrapping_add(i as u8), block, pc)?);
    }
    let result = lower_method_dispatch(ssa, insts, globals, receiver, receiver_ty, &name, &args, block, pc)?;
    ssa.write(base, block, result);
    Ok(())
}

/// `xs.map(|x| …)` / `filter` / `reduce(init, |acc, x| …)` over `List<i64>`
/// with a zero-capture lambda: the compiled `@lk_fn_N` address is passed to an
/// lkrt fold helper. The lambda's signature is seeded/enforced through the
/// same monomorphization lattice as direct calls (`i64 → i64` for map,
/// `i64 → Bool` for filter, `(i64, i64) → i64` for reduce). Returns
/// `Ok(None)` when the shape doesn't apply (the generic path then rejects
/// loudly — never a silent semantic change).
#[allow(clippy::too_many_arguments)]
fn lower_list_hof_k(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    funcs: &[FunctionData],
    entry: u32,
    sig: &mut SigInfer,
    receiver: ValueId,
    name: &str,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<Option<Reg>, Unsupported> {
    let lambda_at = |ssa: &Ssa, reg: u8| match ssa.builtin_regs.get(&(block, reg)) {
        Some(GlobalRef::Lambda(fidx)) => Some(*fidx as usize),
        _ => None,
    };
    let seed_params = |sig: &mut SigInfer, fidx: usize, arity: usize| {
        for i in 0..arity {
            if let Some(slot) = sig.param_obs.get_mut(fidx).and_then(|p| p.get_mut(i)) {
                match slot {
                    None => *slot = Some(Ty::I64),
                    Some(prev) if *prev != Ty::I64 => sig.conflict = true,
                    Some(_) => {}
                }
            }
        }
    };
    match (name, argc) {
        ("map" | "filter", 1) => {
            let Some(fidx) = lambda_at(ssa, base.wrapping_add(1)) else {
                return Ok(None);
            };
            if fidx >= funcs.len() || fidx == entry as usize || funcs[fidx].param_count != 1 {
                return Err(Unsupported::Opcode {
                    pc,
                    op: Opcode::CallMethodK,
                });
            }
            seed_params(sig, fidx, 1);
            let want_ret = if name == "map" { Ty::I64 } else { Ty::Bool };
            if sig.ret_types.get(fidx).copied() != Some(want_ret) {
                // Transiently wrong before the fixpoint converges; final pass
                // rejects real mismatches loudly.
                return Err(Unsupported::TypeMismatch { pc });
            }
            let fnaddr = ssa.new_val();
            insts.push(Inst::Const {
                dst: fnaddr,
                value: Const::FnAddr(FuncId(fidx as u32)),
            });
            let dst = ssa.new_val();
            let hof = if name == "map" { "i64_map_fn" } else { "i64_filter_fn" };
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", hof),
                args: vec![receiver, fnaddr],
            });
            Ok(Some((dst, Ty::ListI64)))
        }
        ("reduce", 2) => {
            let Some(fidx) = lambda_at(ssa, base.wrapping_add(2)) else {
                return Ok(None);
            };
            if fidx >= funcs.len() || fidx == entry as usize || funcs[fidx].param_count != 2 {
                return Err(Unsupported::Opcode {
                    pc,
                    op: Opcode::CallMethodK,
                });
            }
            let init = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
            seed_params(sig, fidx, 2);
            if sig.ret_types.get(fidx).copied() != Some(Ty::I64) {
                return Err(Unsupported::TypeMismatch { pc });
            }
            let fnaddr = ssa.new_val();
            insts.push(Inst::Const {
                dst: fnaddr,
                value: Const::FnAddr(FuncId(fidx as u32)),
            });
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_reduce_fn"),
                args: vec![receiver, init, fnaddr],
            });
            Ok(Some((dst, Ty::I64)))
        }
        _ => Ok(None),
    }
}

/// The shared per-(receiver type, method name, argument types) dispatch table.
#[allow(clippy::too_many_arguments)]
fn lower_method_dispatch(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    receiver: ValueId,
    receiver_ty: Ty,
    name: &str,
    args: &[(ValueId, Ty)],
    block: usize,
    pc: usize,
) -> Result<Reg, Unsupported> {
    let result: Reg = match (receiver_ty, name, args) {
        // `s.starts_with(prefix)` — byte-prefix test, exactly Rust/VM semantics.
        (Ty::Str, "starts_with", [(prefix, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "starts_with"),
                args: vec![receiver, *prefix],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: dst,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        // `s.contains(needle)` — byte-substring test, exactly Rust/VM semantics.
        (Ty::Str, "contains", [(needle, Ty::Str)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "contains"),
                args: vec![receiver, *needle],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: dst,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        // `s.len()` — Unicode scalar count (the VM's `chars().count()`).
        (Ty::Str, "len", []) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "char_len"),
                args: vec![receiver],
            });
            (dst, Ty::I64)
        }
        // `m.get(key)` on string-keyed maps: the missing-key `Maybe` model.
        (Ty::MapStrI64 | Ty::MapStrF64 | Ty::MapStrBool, "get", [(key, Ty::Str)]) => {
            let dst = ssa.new_val();
            let maybe_ty = match receiver_ty {
                Ty::MapStrF64 => {
                    insts.push(Inst::MapGetMaybeStrF64 {
                        dst,
                        handle: receiver,
                        key: *key,
                    });
                    Ty::MaybeF64
                }
                Ty::MapStrBool => {
                    insts.push(Inst::MapGetMaybe {
                        dst,
                        handle: receiver,
                        key: *key,
                    });
                    Ty::MaybeBool
                }
                _ => {
                    insts.push(Inst::MapGetMaybe {
                        dst,
                        handle: receiver,
                        key: *key,
                    });
                    Ty::MaybeI64
                }
            };
            (dst, maybe_ty)
        }
        // `m.set(key, value)` on string-keyed maps.
        (Ty::MapStrI64, "set", [(key, Ty::Str), (value, Ty::I64)])
        | (Ty::MapStrBool, "set", [(key, Ty::Str), (value, Ty::I64)]) => {
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("map_h", "str_i64_set"),
                args: vec![receiver, *key, *value],
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            (nil, Ty::Nil)
        }
        (Ty::MapStrF64, "set", [(key, Ty::Str), (value, Ty::F64)]) => {
            insts.push(Inst::Call {
                dst: None,
                callee: AbiRef::new("map_h", "str_f64_set"),
                args: vec![receiver, *key, *value],
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            (nil, Ty::Nil)
        }
        // `xs.contains(v)` on typed lists (fcmp semantics for f64, like the VM).
        (Ty::ListI64, "contains", [(v, Ty::I64)]) => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("list_h", "i64_contains"),
                args: vec![receiver, *v],
            });
            let zero = ssa.new_val();
            insts.push(Inst::Const {
                dst: zero,
                value: Const::I64(0),
            });
            let b = ssa.new_val();
            insts.push(Inst::Cmp {
                dst: b,
                op: CmpOp::Ne,
                float: false,
                lhs: dst,
                rhs: zero,
            });
            (b, Ty::Bool)
        }
        _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
    };
    let _ = globals;
    let _ = block;
    Ok(result)
}

/// Lowers a `module.method(args)` call whose member [`module_call_abi`] maps to
/// a typed lkrt ABI entry. Arity and argument types must match the schema
/// exactly; the result (or nil) is written to the call-window base register.
#[allow(clippy::too_many_arguments)]
fn lower_module_call(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    module: &str,
    name: &str,
    base: u8,
    argc: usize,
    block: usize,
    pc: usize,
) -> Result<(), Unsupported> {
    // `math.floor`/`ceil`/`round` dispatch on the argument's static type,
    // matching the VM's `integer_round`: an `Int` passes through unchanged, a
    // `Float` rounds via the lkrt helper (`f64::xxx() as i64`).
    if module == "math" && matches!(name, "floor" | "ceil" | "round") {
        if argc != 1 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        let (v, ty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        match ty {
            Ty::I64 => ssa.write(base, block, (v, Ty::I64)),
            Ty::F64 => {
                let round_fn = match name {
                    "floor" => "floor",
                    "ceil" => "ceil",
                    _ => "round",
                };
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("math", round_fn),
                    args: vec![v],
                });
                ssa.write(base, block, (dst, Ty::I64));
            }
            _ => return Err(Unsupported::TypeMismatch { pc }),
        }
        return Ok(());
    }
    // `math.abs` returns its argument's type: Int → wrapping integer abs
    // (select(x < 0, 0 - x, x), sub wraps like the VM's release build),
    // Float → fabs via select on the float compare.
    if module == "math" && name == "abs" {
        if argc != 1 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        let (v, ty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        if !matches!(ty, Ty::I64 | Ty::F64) {
            return Err(Unsupported::TypeMismatch { pc });
        }
        let zero = ssa.new_val();
        insts.push(Inst::Const {
            dst: zero,
            value: if ty == Ty::F64 { Const::F64(0.0) } else { Const::I64(0) },
        });
        let negative = ssa.new_val();
        insts.push(Inst::Cmp {
            dst: negative,
            op: CmpOp::Lt,
            float: ty == Ty::F64,
            lhs: v,
            rhs: zero,
        });
        let negated = ssa.new_val();
        if ty == Ty::F64 {
            insts.push(Inst::FloatBin {
                dst: negated,
                op: FloatBinOp::Sub,
                lhs: zero,
                rhs: v,
            });
        } else {
            insts.push(Inst::IntBin {
                dst: negated,
                op: IntBinOp::Sub,
                lhs: zero,
                rhs: v,
            });
        }
        let dst = ssa.new_val();
        insts.push(Inst::Select {
            dst,
            cond: negative,
            then_v: negated,
            else_v: v,
            ty,
        });
        ssa.write(base, block, (dst, ty));
        return Ok(());
    }
    // `io.std` (bound as the `std` global by `use { std } from io`): the
    // stdio resources are fixed handles (stdin 0 / stdout 1 / stderr 2 — the
    // lkrt convention); `write`/`writeln` return the VM's written byte count,
    // `flush` is always `true` on success (errors abort loudly on both sides).
    if module == "std" {
        match name {
            "stdin" | "stdout" | "stderr" => {
                if argc != 0 {
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
                }
                let handle = match name {
                    "stdin" => 0,
                    "stdout" => 1,
                    _ => 2,
                };
                let dst = ssa.new_val();
                insts.push(Inst::Const {
                    dst,
                    value: Const::I64(handle),
                });
                ssa.write(base, block, (dst, Ty::I64));
                return Ok(());
            }
            "write" | "writeln" => {
                if argc != 2 {
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
                }
                let handle = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                let data = ssa.read_typed(base.wrapping_add(2), block, Ty::Str, pc)?;
                let newline = ssa.new_val();
                insts.push(Inst::Const {
                    dst: newline,
                    value: Const::I64(i64::from(name == "writeln")),
                });
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("io.std", "write"),
                    args: vec![handle, data, newline],
                });
                ssa.write(base, block, (dst, Ty::I64));
                return Ok(());
            }
            "flush" => {
                if argc != 1 {
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
                }
                let handle = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("io.std", "flush"),
                    args: vec![handle],
                });
                let dst = ssa.new_val();
                insts.push(Inst::Const {
                    dst,
                    value: Const::Bool(true),
                });
                ssa.write(base, block, (dst, Ty::Bool));
                return Ok(());
            }
            "read_to_string" => {
                if argc != 1 {
                    return Err(Unsupported::Opcode { pc, op: Opcode::Call });
                }
                let handle = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("io.std", "read_to_string"),
                    args: vec![handle],
                });
                ssa.write(base, block, (dst, Ty::Str));
                return Ok(());
            }
            _ => return Err(Unsupported::Opcode { pc, op: Opcode::Call }),
        }
    }
    // `datetime.add`/`sub` are plain Int arithmetic (`timestamp ± seconds`);
    // `is_weekend` returns the helper's 0/1 as a `Bool`.
    if module == "datetime" && matches!(name, "add" | "sub") {
        if argc != 2 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        let ts = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
        let secs = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
        let dst = ssa.new_val();
        insts.push(Inst::IntBin {
            dst,
            op: if name == "add" { IntBinOp::Add } else { IntBinOp::Sub },
            lhs: ts,
            rhs: secs,
        });
        ssa.write(base, block, (dst, Ty::I64));
        return Ok(());
    }
    if module == "datetime" && name == "is_weekend" {
        if argc != 1 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        let ts = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
        let wide = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(wide),
            callee: AbiRef::new("datetime", "is_weekend"),
            args: vec![ts],
        });
        let zero = ssa.new_val();
        insts.push(Inst::Const {
            dst: zero,
            value: Const::I64(0),
        });
        let dst = ssa.new_val();
        insts.push(Inst::Cmp {
            dst,
            op: CmpOp::Ne,
            float: false,
            lhs: wide,
            rhs: zero,
        });
        ssa.write(base, block, (dst, Ty::Bool));
        return Ok(());
    }
    // `time.since(start, end)` is `end - start` (the VM's `numeric_millis`
    // subtraction); Int-typed millisecond values only — Float coercion stays
    // out of the subset.
    if module == "time" && name == "since" {
        if argc != 2 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        let start = read_typed_scalar(ssa, insts, base.wrapping_add(1), block, Ty::I64, pc)?;
        let end = read_typed_scalar(ssa, insts, base.wrapping_add(2), block, Ty::I64, pc)?;
        let dst = ssa.new_val();
        insts.push(Inst::IntBin {
            dst,
            op: IntBinOp::Sub,
            lhs: end,
            rhs: start,
        });
        ssa.write(base, block, (dst, Ty::I64));
        return Ok(());
    }
    // `math.min`/`max` return one of the *original* arguments (comparison per
    // the VM's `min_max`); same-type scalar pairs lower to a select.
    if module == "math" && matches!(name, "min" | "max") {
        if argc != 2 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        let (l, lty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        let (r, rty) = read_scalar(ssa, insts, base.wrapping_add(2), block, pc)?;
        if lty != rty || !matches!(lty, Ty::I64 | Ty::F64) {
            return Err(Unsupported::TypeMismatch { pc });
        }
        let pick_left = ssa.new_val();
        insts.push(Inst::Cmp {
            dst: pick_left,
            op: if name == "min" { CmpOp::Lt } else { CmpOp::Gt },
            float: lty == Ty::F64,
            lhs: l,
            rhs: r,
        });
        let dst = ssa.new_val();
        insts.push(Inst::Select {
            dst,
            cond: pick_left,
            then_v: l,
            else_v: r,
            ty: lty,
        });
        ssa.write(base, block, (dst, lty));
        return Ok(());
    }
    let Some((callee, param_tys, ret_ty)) = module_call_abi(module, name) else {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    };
    if argc != param_tys.len() {
        return Err(Unsupported::Opcode { pc, op: Opcode::Call });
    }
    let mut args = Vec::with_capacity(argc);
    for (i, want) in param_tys.iter().enumerate() {
        let arg_reg = base.wrapping_add(1).wrapping_add(i as u8);
        // `Number` parameters (schema type F64) accept an Int by promotion,
        // matching the stdlib module's `number_arg` coercion.
        if *want == Ty::F64 {
            let (v, ty) = read_scalar(ssa, insts, arg_reg, block, pc)?;
            match ty {
                Ty::F64 => args.push(v),
                Ty::I64 => {
                    let f = ssa.new_val();
                    insts.push(Inst::IntToFloat { dst: f, src: v });
                    args.push(f);
                }
                _ => return Err(Unsupported::TypeMismatch { pc }),
            }
            continue;
        }
        args.push(ssa.read_typed(arg_reg, block, *want, pc)?);
    }
    let dst = match ret_ty {
        Ty::Nil => {
            insts.push(Inst::Call {
                dst: None,
                callee,
                args,
            });
            let nil = ssa.new_val();
            insts.push(Inst::Const {
                dst: nil,
                value: Const::Nil,
            });
            (nil, Ty::Nil)
        }
        _ => {
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee,
                args,
            });
            (dst, ret_ty)
        }
    };
    ssa.write(base, block, dst);
    Ok(())
}

/// Assembles the output pieces of a `print`/`println` call, mirroring the VM's
/// `format_variadic_runtime` exactly:
///  - no args → empty output;
///  - a *constant* string first arg is the format: each `{}` consumes the next
///    arg's display, leftover `{}` stay literal, and leftover args append
///    space-separated with a leading space iff the rendered format part is
///    non-empty (decided statically; the one runtime-dependent case — only
///    `Str` placeholders and no literal text — rejects);
///  - a dynamic (non-constant) `Str` first arg lowers only as the sole
///    argument (the output is then the string itself, `{}` included);
///  - a non-string first arg joins all args' displays with single spaces.
fn print_parts(ssa: &mut Ssa, base: u8, argc: usize, block: usize, pc: usize) -> Result<Vec<PrintPart>, Unsupported> {
    let mut args = Vec::with_capacity(argc);
    for i in 0..argc {
        args.push(ssa.read(base.wrapping_add(1).wrapping_add(i as u8), block, pc)?);
    }
    let Some((&(first_v, first_ty), _)) = args.split_first() else {
        return Ok(Vec::new());
    };
    if first_ty != Ty::Str {
        let mut parts = Vec::new();
        for (i, &(v, ty)) in args.iter().enumerate() {
            if i > 0 {
                parts.push(PrintPart::Lit(" ".to_string()));
            }
            parts.push(PrintPart::Val(v, ty));
        }
        return Ok(parts);
    }
    let const_fmt = ssa
        .const_strs
        .get(&first_v)
        .cloned()
        // Loop bodies read the format through a loop-header phi (the compiler
        // hoists loop literals); recover the constant via reaching definitions.
        .or_else(|| ssa.reg_const_str(base.wrapping_add(1), block));
    let Some(fmt) = const_fmt else {
        if argc == 1 {
            return Ok(vec![PrintPart::Val(first_v, Ty::Str)]);
        }
        return Err(Unsupported::TypeMismatch { pc });
    };
    let rest = &args[1..];
    let mut parts: Vec<PrintPart> = Vec::new();
    let mut lit = String::new();
    let mut chars = fmt.chars().peekable();
    let mut next_arg = 0usize;
    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            chars.next();
            if let Some(&(v, ty)) = rest.get(next_arg) {
                if !lit.is_empty() {
                    parts.push(PrintPart::Lit(std::mem::take(&mut lit)));
                }
                parts.push(PrintPart::Val(v, ty));
                next_arg += 1;
            } else {
                lit.push_str("{}");
            }
        } else {
            lit.push(ch);
        }
    }
    if !lit.is_empty() {
        parts.push(PrintPart::Lit(std::mem::take(&mut lit)));
    }
    let extras = &rest[next_arg..];
    if !extras.is_empty() {
        // The VM inserts one space iff the rendered format part is non-empty.
        // Statically: literal pieces are non-empty by construction and
        // i64/f64/bool displays are never empty; only a `Str` placeholder can
        // render empty, which makes the space runtime-dependent → reject.
        if !parts.is_empty() {
            let has_lit = parts.iter().any(|p| matches!(p, PrintPart::Lit(_)));
            let str_placeholder = parts.iter().any(|p| matches!(p, PrintPart::Val(_, Ty::Str)));
            if !has_lit && str_placeholder {
                return Err(Unsupported::TypeMismatch { pc });
            }
            parts.push(PrintPart::Lit(" ".to_string()));
        }
        for (i, &(v, ty)) in extras.iter().enumerate() {
            if i > 0 {
                parts.push(PrintPart::Lit(" ".to_string()));
            }
            parts.push(PrintPart::Val(v, ty));
        }
    }
    Ok(parts)
}

/// Renders assembled [`PrintPart`]s: adjacent literals merge into one interned
/// global, value parts display-convert, everything folds into a single string
/// via `str.concat` (freeing consumed temporaries), and one [`Inst::PrintStr`]
/// emits it.
fn emit_print(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    parts: Vec<PrintPart>,
    newline: bool,
    pc: usize,
) -> Result<(), Unsupported> {
    fn lit_value(ssa: &mut Ssa, insts: &mut Vec<Inst>, globals: &mut Vec<String>, text: &str) -> ValueId {
        let gid = intern_global(globals, text);
        let dst = ssa.new_val();
        insts.push(Inst::Const {
            dst,
            value: Const::Str(GlobalId(gid)),
        });
        dst
    }

    let mut pieces: Vec<(ValueId, bool)> = Vec::new();
    let mut pending = String::new();
    for part in parts {
        match part {
            PrintPart::Lit(s) => pending.push_str(&s),
            PrintPart::Val(v, ty) => {
                if !pending.is_empty() {
                    let lit = lit_value(ssa, insts, globals, &pending);
                    pieces.push((lit, false));
                    pending.clear();
                }
                pieces.push(to_display_str(ssa, insts, globals, v, ty, true, pc)?);
            }
        }
    }
    if !pending.is_empty() {
        let lit = lit_value(ssa, insts, globals, &pending);
        pieces.push((lit, false));
    }

    let (value, fresh) = match pieces.split_first() {
        None => (lit_value(ssa, insts, globals, ""), false),
        Some((&(first, first_fresh), rest)) => {
            let mut acc = first;
            let mut acc_fresh = first_fresh;
            for &(v, v_fresh) in rest {
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("str", "concat"),
                    args: vec![acc, v],
                });
                if acc_fresh {
                    free_owned_str(insts, acc);
                }
                if v_fresh {
                    free_owned_str(insts, v);
                }
                acc = dst;
                acc_fresh = true;
            }
            (acc, acc_fresh)
        }
    };
    insts.push(Inst::PrintStr { value, newline });
    if fresh {
        free_owned_str(insts, value);
    }
    Ok(())
}

/// Materializes a constant map key as a `Str` value (an interned global) for the
/// map ABI, which takes the key as a `*const c_char`.
/// Boxes a typed runtime value into a `Dyn` carrier (plan M4.2): identity
/// for `Ty::Dyn`, a `dyn.from_*` call for scalars/strings/mixed lists.
/// Types without a boxed form (Maybe carriers, typed containers) reject —
/// their typed paths stay typed.
fn to_dyn(ssa: &mut Ssa, insts: &mut Vec<Inst>, v: ValueId, ty: Ty, pc: usize) -> Result<ValueId, Unsupported> {
    let from = match ty {
        Ty::Dyn => return Ok(v),
        Ty::I64 => "from_i64",
        Ty::F64 => "from_f64",
        Ty::Str => "from_str",
        Ty::Nil => "from_nil",
        Ty::ListDyn => "from_list",
        Ty::Bool => {
            let wide = ssa.new_val();
            insts.push(Inst::ZextBool { dst: wide, src: v });
            let boxed = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_bool"),
                args: vec![wide],
            });
            return Ok(boxed);
        }
        _ => return Err(Unsupported::TypeMismatch { pc }),
    };
    let boxed = ssa.new_val();
    insts.push(Inst::Call {
        dst: Some(boxed),
        callee: AbiRef::new("dyn", from),
        args: if ty == Ty::Nil { Vec::new() } else { vec![v] },
    });
    Ok(boxed)
}

/// Boxes one constant scalar into a `Dyn` carrier value (plan M4.2): emits
/// the scalar `Const` plus the matching `dyn.from_*` call. Callers filtered
/// to scalar variants.
fn box_const_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    value: &ConstRuntimeValueData,
) -> ValueId {
    let boxed = ssa.new_val();
    match value {
        ConstRuntimeValueData::Nil => {
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_nil"),
                args: Vec::new(),
            });
        }
        ConstRuntimeValueData::Bool(b) => {
            let raw = ssa.new_val();
            insts.push(Inst::Const {
                dst: raw,
                value: Const::I64(i64::from(*b)),
            });
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_bool"),
                args: vec![raw],
            });
        }
        ConstRuntimeValueData::Int(n) => {
            let raw = ssa.new_val();
            insts.push(Inst::Const {
                dst: raw,
                value: Const::I64(*n),
            });
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_i64"),
                args: vec![raw],
            });
        }
        ConstRuntimeValueData::Float(x) => {
            let raw = ssa.new_val();
            insts.push(Inst::Const {
                dst: raw,
                value: Const::F64(*x),
            });
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_f64"),
                args: vec![raw],
            });
        }
        ConstRuntimeValueData::ShortStr(s) => {
            let raw = ssa.new_val();
            insts.push(Inst::Const {
                dst: raw,
                value: Const::Str(GlobalId(intern_global(globals, s))),
            });
            insts.push(Inst::Call {
                dst: Some(boxed),
                callee: AbiRef::new("dyn", "from_str"),
                args: vec![raw],
            });
        }
        ConstRuntimeValueData::Heap(_) => unreachable!("callers filter to scalar variants"),
    }
    boxed
}

fn materialize_key(ssa: &mut Ssa, insts: &mut Vec<Inst>, globals: &mut Vec<String>, key: &str) -> ValueId {
    let gid = intern_global(globals, key);
    let dst = ssa.new_val();
    insts.push(Inst::Const {
        dst,
        value: Const::Str(GlobalId(gid)),
    });
    dst
}

/// Reads `list[index]` as an `i64` **scalar** (for fused list-arithmetic opcodes): a
/// provably in-range constant index folds to a clean `at`; otherwise it goes through
/// the Maybe read + present-asserting unwrap, which aborts on an out-of-range or
/// too-negative index — exactly matching the VM's `read_known_int_list_index`
/// (negative counts from the end, else the access is a fatal halt).
fn list_i64_element_scalar(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    list_reg: u8,
    index_reg: u8,
    block: usize,
    pc: usize,
) -> Result<ValueId, Unsupported> {
    let (handle, list_ty) = ssa.read(list_reg, block, pc)?;
    if list_ty != Ty::ListI64 {
        return Err(Unsupported::TypeMismatch { pc });
    }
    let index = read_typed_scalar(ssa, insts, index_reg, block, Ty::I64, pc)?;
    let const_in_range = match (ssa.list_len.get(&handle), ssa.const_int.get(&index)) {
        (Some(&len), Some(&idx)) if idx >= 0 && idx < len => Some(idx),
        _ => None,
    };
    if let Some(idx) = const_in_range {
        let idx_v = ssa.new_val();
        insts.push(Inst::Const {
            dst: idx_v,
            value: Const::I64(idx),
        });
        let d = ssa.new_val();
        insts.push(Inst::Call {
            dst: Some(d),
            callee: AbiRef::new("list_h", "i64_at"),
            args: vec![handle, idx_v],
        });
        Ok(d)
    } else {
        let m = ssa.new_val();
        insts.push(Inst::ListGetMaybe { dst: m, handle, index });
        let d = ssa.new_val();
        insts.push(Inst::UnwrapMaybeI64 { dst: d, src: m });
        Ok(d)
    }
}

/// The float form of an `AddInt`/…/`ModInt` opcode (used when runtime dispatch
/// selects float arithmetic for a float/mixed operand pair).
fn int_to_float_bin_op(op: Opcode) -> FloatBinOp {
    match op {
        Opcode::AddInt => FloatBinOp::Add,
        Opcode::SubInt => FloatBinOp::Sub,
        Opcode::MulInt => FloatBinOp::Mul,
        Opcode::DivInt => FloatBinOp::Div,
        Opcode::ModInt => FloatBinOp::Mod,
        _ => unreachable!("integer arithmetic opcode"),
    }
}

fn float_bin_op(op: Opcode) -> FloatBinOp {
    match op {
        Opcode::AddFloat => FloatBinOp::Add,
        Opcode::SubFloat => FloatBinOp::Sub,
        Opcode::MulFloat => FloatBinOp::Mul,
        Opcode::DivFloat => FloatBinOp::Div,
        Opcode::ModFloat => FloatBinOp::Mod,
        _ => unreachable!("float arithmetic opcode"),
    }
}

fn cmp_op(op: Opcode) -> CmpOp {
    match op {
        Opcode::CmpInt => CmpOp::Eq,
        Opcode::CmpNeInt => CmpOp::Ne,
        Opcode::CmpLtInt => CmpOp::Lt,
        Opcode::CmpLeInt => CmpOp::Le,
        Opcode::CmpGtInt => CmpOp::Gt,
        Opcode::CmpGeInt => CmpOp::Ge,
        _ => unreachable!("integer compare opcode"),
    }
}

fn test_cmp_op(op: Opcode) -> Option<CmpOp> {
    Some(match op {
        Opcode::TestEqInt | Opcode::TestEqIntI => CmpOp::Eq,
        Opcode::TestNeInt | Opcode::TestNeIntI => CmpOp::Ne,
        Opcode::TestLtInt | Opcode::TestLtIntI => CmpOp::Lt,
        Opcode::TestLeInt | Opcode::TestLeIntI => CmpOp::Le,
        Opcode::TestGtInt | Opcode::TestGtIntI => CmpOp::Gt,
        Opcode::TestGeInt | Opcode::TestGeIntI => CmpOp::Ge,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use lk_core::vm::{ConstPoolData, FunctionData, MODULE_ARTIFACT_VERSION, ModuleData};

    fn artifact(consts: ConstPoolData, code: Vec<u32>, register_count: u16) -> ModuleArtifact {
        ModuleArtifact {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports: Vec::new(),
            module: ModuleData {
                entry: 0,
                globals: Vec::new(),
                functions: vec![FunctionData {
                    consts,
                    code,
                    performance: Default::default(),
                    register_count,
                    param_count: 0,
                    positional_param_count: 0,
                    param_names: Vec::new(),
                    capture_count: 0,
                    debug_name: None,
                }],
            },
        }
    }

    fn ints(v: Vec<i64>) -> ConstPoolData {
        ConstPoolData {
            ints: v,
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: Vec::new(),
        }
    }

    fn floats(v: Vec<f64>) -> ConstPoolData {
        ConstPoolData {
            ints: Vec::new(),
            floats: v,
            strings: Vec::new(),
            heap_values: Vec::new(),
        }
    }

    fn func(consts: ConstPoolData, code: Vec<u32>, register_count: u16, param_count: u16) -> FunctionData {
        FunctionData {
            consts,
            code,
            performance: Default::default(),
            register_count,
            param_count,
            positional_param_count: param_count,
            param_names: Vec::new(),
            capture_count: 0,
            debug_name: None,
        }
    }

    /// `let inc = |x| x + 1; return inc(41);` — a zero-capture closure stored in
    /// a module global (entry-prefix single assignment), read back and called
    /// indirectly: devirtualizes to a direct `CallFn`.
    #[test]
    fn lowers_zero_capture_lambda_global_call() {
        let art = ModuleArtifact {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports: Vec::new(),
            module: ModuleData {
                entry: 0,
                globals: vec!["inc".to_string()],
                functions: vec![
                    func(
                        ints(vec![41]),
                        vec![
                            Instr::abc(Opcode::MakeClosure, 0, 1, 0).raw(),
                            Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                            Instr::abx(Opcode::GetGlobal, 1, 0).raw(),
                            Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                            Instr::abc(Opcode::Call, 1, 0, 1).raw(),
                            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                        ],
                        4,
                        0,
                    ),
                    func(
                        ints(vec![1]),
                        vec![
                            Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                            Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),
                            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
                        ],
                        3,
                        1,
                    ),
                ],
            },
        };
        let mir = lower(&art).expect("zero-capture lambda lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert_eq!(mir.functions.len(), 2, "lambda body must be reachable/emitted");
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call i64 @lk_fn_1"), "devirtualized direct call: {ir}");
    }

    /// A register-local zero-capture lambda (no global storage) calls directly
    /// through the tracked `MakeClosure` ref.
    #[test]
    fn lowers_local_lambda_call() {
        let art = ModuleArtifact {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports: Vec::new(),
            module: ModuleData {
                entry: 0,
                globals: Vec::new(),
                functions: vec![
                    func(
                        ints(vec![20]),
                        vec![
                            Instr::abc(Opcode::MakeClosure, 1, 1, 0).raw(),
                            Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                            Instr::abc(Opcode::Call, 1, 0, 1).raw(),
                            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                        ],
                        4,
                        0,
                    ),
                    func(
                        ints(vec![2]),
                        vec![
                            Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                            Instr::abc(Opcode::MulInt, 2, 0, 1).raw(),
                            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
                        ],
                        3,
                        1,
                    ),
                ],
            },
        };
        let mir = lower(&art).expect("local lambda lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert_eq!(mir.functions.len(), 2);
    }

    /// A capturing closure (`capture_count == 1`) rejects the module.
    #[test]
    fn rejects_capturing_closure() {
        let mut lambda = func(ints(vec![]), vec![Instr::abc(Opcode::Return1, 0, 0, 0).raw()], 2, 1);
        lambda.capture_count = 1;
        let art = ModuleArtifact {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports: Vec::new(),
            module: ModuleData {
                entry: 0,
                globals: Vec::new(),
                functions: vec![
                    func(
                        ints(vec![]),
                        vec![
                            Instr::abc(Opcode::MakeClosure, 0, 1, 0).raw(),
                            Instr::abc(Opcode::Return1, 0, 0, 0).raw(),
                        ],
                        2,
                        0,
                    ),
                    lambda,
                ],
            },
        };
        assert!(lower(&art).is_err(), "capturing closure must reject");
    }

    /// A lambda global written twice is not single-assignment: readers could
    /// observe either closure, so the module rejects loudly.
    #[test]
    fn rejects_reassigned_lambda_global() {
        let lambda_body = |k: i64| {
            func(
                ints(vec![k]),
                vec![
                    Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                    Instr::abc(Opcode::MulInt, 2, 0, 1).raw(),
                    Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
                ],
                3,
                1,
            )
        };
        let art = ModuleArtifact {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports: Vec::new(),
            module: ModuleData {
                entry: 0,
                globals: vec!["f".to_string()],
                functions: vec![
                    func(
                        ints(vec![5]),
                        vec![
                            Instr::abc(Opcode::MakeClosure, 0, 1, 0).raw(),
                            Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                            Instr::abc(Opcode::MakeClosure, 0, 2, 0).raw(),
                            Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                            Instr::abx(Opcode::GetGlobal, 1, 0).raw(),
                            Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                            Instr::abc(Opcode::Call, 1, 0, 1).raw(),
                            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                        ],
                        4,
                        0,
                    ),
                    lambda_body(1),
                    lambda_body(2),
                ],
            },
        };
        assert!(lower(&art).is_err(), "reassigned lambda global must reject");
    }

    /// `fn add(x,y){ return x+y } return add(3,4)` — a two-function module with a
    /// register-window direct call (`CallDirect a=1 b=1 c=2`, args at r2/r3).
    #[test]
    fn lowers_direct_call() {
        let art = ModuleArtifact {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports: Vec::new(),
            module: ModuleData {
                entry: 0,
                globals: vec!["add".to_string()],
                functions: vec![
                    func(
                        ints(vec![3, 4]),
                        vec![
                            Instr::abx(Opcode::LoadFunction, 0, 1).raw(),
                            Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                            Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                            Instr::abx(Opcode::LoadInt, 3, 1).raw(),
                            Instr::abc(Opcode::CallDirect, 1, 1, 2).raw(),
                            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                        ],
                        4,
                        0,
                    ),
                    func(
                        ints(vec![]),
                        vec![
                            Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),
                            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
                        ],
                        3,
                        2,
                    ),
                ],
            },
        };
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert_eq!(mir.functions.len(), 2);
        // fn 1 is (i64, i64) -> i64.
        assert_eq!(mir.functions[1].params.len(), 2);
        assert_eq!(mir.functions[1].ret, Ty::I64);
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("define i64 @lk_fn_1(i64 %v0, i64 %v1)"), "{ir}");
        assert!(ir.contains("call i64 @lk_fn_1(i64 %v"), "{ir}");
        // The callee body adds its two params and returns the sum.
        assert!(ir.contains("add i64 %v0, %v1"), "{ir}");
    }

    /// `["a","b","c"].join("-")` — a constant `List<str>` materializes a str-list
    /// handle (new + str_push) and `ListJoin` lowers to `str_join`.
    #[test]
    fn lowers_str_list_join() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: Vec::new(),
            floats: Vec::new(),
            strings: vec!["-".to_string()],
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::ShortStr("a".to_string()),
                ConstRuntimeValueData::ShortStr("b".to_string()),
                ConstRuntimeValueData::ShortStr("c".to_string()),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = ["a","b","c"]
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
                Instr::abx(Opcode::LoadString, 1, 0).raw(),    // r1 = "-"
                Instr::abc(Opcode::ListJoin, 2, 0, 1).raw(),   // r2 = xs.join("-")
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("str list + join lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert_eq!(ir.matches("call void @lkrt_lklist_str_push(ptr").count(), 3, "{ir}");
        assert!(ir.contains("call ptr @lkrt_lklist_str_join(ptr"), "{ir}");
    }

    /// `2 in xs` (`Contains a=dst b=needle c=haystack`) lowers to the list membership
    /// helper, narrowed from the runtime's `0/1` to an `i1`.
    #[test]
    fn lowers_in_operator() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![2],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Int(1),
                ConstRuntimeValueData::Int(2),
                ConstRuntimeValueData::Int(3),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = [1,2,3]
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 2 (needle)
                Instr::abc(Opcode::Contains, 2, 1, 0).raw(),   // r2 = (2 in xs)
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("in-operator lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call i64 @lkrt_lklist_i64_contains(ptr"), "{ir}");
        assert!(ir.contains("icmp ne i64"), "narrowed to bool: {ir}");
    }

    /// A defined-but-never-called function (here with an unsupported out-of-range
    /// constant) is dead for AOT: it is skipped, so it does not fail the module, and
    /// only the entry is emitted.
    #[test]
    fn dead_function_is_skipped() {
        let art = ModuleArtifact {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports: Vec::new(),
            module: ModuleData {
                entry: 0,
                globals: vec!["dead".to_string()],
                functions: vec![
                    func(
                        ints(vec![42]),
                        vec![
                            Instr::abx(Opcode::LoadFunction, 0, 1).raw(), // registers `dead`, never calls it
                            Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                            Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                        ],
                        2,
                        0,
                    ),
                    // `dead`: an out-of-range const would be `Unsupported`, but it is
                    // never reached, so it must not fail the module.
                    func(
                        ints(vec![]),
                        vec![
                            Instr::abx(Opcode::LoadInt, 0, 99).raw(),
                            Instr::abc(Opcode::Return1, 0, 0, 0).raw(),
                        ],
                        1,
                        0,
                    ),
                ],
            },
        };
        let mir = lower(&art).expect("dead function does not block lowering");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert_eq!(mir.functions.len(), 1, "only the reachable entry is emitted");
        assert_eq!(mir.functions[0].id, FuncId(0));
    }

    /// `fn f(x){ return x } return f(4.0)` — the callee's parameter type is
    /// monomorphized to `F64` from the (single, consistent) `f64` call site, so `f`
    /// lowers as `double @lk_fn_1(double)`.
    #[test]
    fn monomorphizes_f64_parameter() {
        let art = ModuleArtifact {
            format: "lk.module".to_string(),
            version: MODULE_ARTIFACT_VERSION,
            imports: Vec::new(),
            module: ModuleData {
                entry: 0,
                globals: vec!["f".to_string()],
                functions: vec![
                    func(
                        floats(vec![4.0]),
                        vec![
                            Instr::abx(Opcode::LoadFunction, 0, 1).raw(),
                            Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                            Instr::abx(Opcode::LoadFloat, 2, 0).raw(), // r2 = 4.0
                            Instr::abc(Opcode::CallDirect, 1, 1, 1).raw(), // r1 = f(r2)
                            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                        ],
                        3,
                        0,
                    ),
                    func(ints(vec![]), vec![Instr::abc(Opcode::Return1, 0, 0, 0).raw()], 1, 1),
                ],
            },
        };
        let mir = lower(&art).expect("lowers with f64 param");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert_eq!(mir.functions[1].params[0].1, Ty::F64);
        assert_eq!(mir.functions[1].ret, Ty::F64);
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("define double @lk_fn_1(double %v0)"), "{ir}");
        assert!(ir.contains("call double @lk_fn_1(double %v"), "{ir}");
    }

    /// `let xs = [10,20,30]; return xs.len();` — a constant `List<i64>` materialized
    /// into a growable `lkrt` handle, then `Len`.
    #[test]
    fn lowers_const_list_len() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: Vec::new(),
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Int(10),
                ConstRuntimeValueData::Int(20),
                ConstRuntimeValueData::Int(30),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),
                Instr::abc(Opcode::Len, 1, 0, 0).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            2,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert_eq!(mir.functions[0].ret, Ty::I64);
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call ptr @lkrt_lklist_i64_new()"), "{ir}");
        assert!(ir.contains("call void @lkrt_lklist_i64_push(ptr"), "{ir}");
        assert!(ir.contains("call i64 @lkrt_lklist_i64_len(ptr"), "{ir}");
    }

    /// `let xs=[10,20,30]; return xs[0];` — a provably in-range constant index on a
    /// const-materialized list lowers to a runtime `lkrt_lklist_i64_at`.
    #[test]
    fn lowers_const_inbounds_index() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![0],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Int(10),
                ConstRuntimeValueData::Int(20),
                ConstRuntimeValueData::Int(30),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),
                Instr::abx(Opcode::LoadInt, 2, 0).raw(), // index 0
                Instr::abc(Opcode::GetList, 1, 0, 2).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert!(lk_aot_codegen::render_module(&mir).contains("call i64 @lkrt_lklist_i64_at(ptr"));
    }

    /// `let xs=[10]; xs.push(20); xs.push(30); return xs[2];` — in-place push grows
    /// the tracked length so a later constant index stays provably in range.
    #[test]
    fn lowers_list_push_then_index() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![20, 30, 2],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![ConstRuntimeValueData::Int(10)])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = [10]
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 20
                Instr::abc(Opcode::ListPush, 0, 1, 0).raw(),   // xs.push(20)
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),       // r1 = 30
                Instr::abc(Opcode::ListPush, 0, 1, 0).raw(),   // xs.push(30)
                Instr::abx(Opcode::LoadInt, 1, 2).raw(),       // r1 = 2 (index)
                Instr::abc(Opcode::GetList, 1, 0, 1).raw(),    // xs[2]
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            2,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert_eq!(
            ir.matches("call void @lkrt_lklist_i64_push(ptr").count(),
            3,
            "1 init + 2 pushes: {ir}"
        );
        assert!(ir.contains("call i64 @lkrt_lklist_i64_at(ptr"), "{ir}");
    }

    /// `let xs=[10,20,30]; xs[1]=99; return xs[1];` — a store lowers to the
    /// bounds-checked `i64_set` helper, and the subsequent in-range read still folds
    /// to a clean `at`.
    #[test]
    fn lowers_set_index() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![1, 99],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Int(10),
                ConstRuntimeValueData::Int(20),
                ConstRuntimeValueData::Int(30),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = [10,20,30]
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 1 (index)
                Instr::abx(Opcode::LoadInt, 2, 1).raw(),       // r2 = 99 (value)
                Instr::abc(Opcode::SetIndex, 0, 1, 2).raw(),   // xs[1] = 99
                Instr::abx(Opcode::LoadInt, 2, 0).raw(),       // r2 = 1 (index)
                Instr::abc(Opcode::GetList, 1, 0, 2).raw(),    // xs[1]
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("lowers set + read");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call void @lkrt_lklist_i64_set(ptr"), "{ir}");
        // The read after the store is provably in range → clean `at`, not a Maybe.
        assert!(ir.contains("call i64 @lkrt_lklist_i64_at(ptr"), "{ir}");
    }

    /// A dynamic index of an `f64` list produces a `MaybeF64`; consumed by a return
    /// it renders the by-value `f64` get-pair and a present-branching print.
    #[test]
    fn dynamic_f64_index_lowers_to_maybe_f64() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![0, 1],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Float(1.5),
                ConstRuntimeValueData::Float(2.5),
            ])],
        };
        // Build a non-constant index (r1 = 0; r1 = r1 + 1) so the access is dynamic.
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 2, 0).raw(), // r2 = [1.5,2.5]
                Instr::abc(Opcode::Move, 0, 2, 0).raw(),       // r0 = xs
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 0
                Instr::abc(Opcode::AddIntI, 1, 1, 1).raw(),    // r1 = r1 + 1 (dynamic)
                Instr::abc(Opcode::GetList, 1, 0, 1).raw(),    // r1 = xs[r1]
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("f64 dynamic index lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(
            ir.contains("call { double, i64 } @lkrt_lklist_f64_get_pair(ptr"),
            "{ir}"
        );
        assert!(ir.contains("@lk_f64_fmt"), "prints f64 element on present: {ir}");
    }

    /// The `str` analogue of the dynamic-index `Maybe` model: a non-constant index
    /// into a `List<str>` lowers to `lkrt_lklist_str_get_pair` (`{ptr, i64}`), and
    /// the returned `Maybe<str>` prints the element or nothing.
    #[test]
    fn dynamic_str_index_lowers_to_maybe_str() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![0, 1],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::ShortStr("foo".to_string()),
                ConstRuntimeValueData::ShortStr("bar".to_string()),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 2, 0).raw(), // r2 = ["foo","bar"]
                Instr::abc(Opcode::Move, 0, 2, 0).raw(),       // r0 = xs
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 0
                Instr::abc(Opcode::AddIntI, 1, 1, 1).raw(),    // r1 = r1 + 1 (dynamic)
                Instr::abc(Opcode::GetList, 1, 0, 1).raw(),    // r1 = xs[r1]
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("str dynamic index lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call { ptr, i64 } @lkrt_lklist_str_get_pair(ptr"), "{ir}");
        assert!(ir.contains("@lk_str_fmt"), "prints str element on present: {ir}");
    }

    /// Fused `acc += list[index]` (`AddListInt`): with a provably in-range constant
    /// index the element folds to a clean `at`, then an integer add.
    #[test]
    fn lowers_add_list_int() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![5, 1],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Int(10),
                ConstRuntimeValueData::Int(20),
                ConstRuntimeValueData::Int(30),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = [10,20,30]
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 5 (acc)
                Instr::abx(Opcode::LoadInt, 2, 1).raw(),       // r2 = 1 (index)
                Instr::abc(Opcode::AddListInt, 1, 0, 2).raw(), // r1 += xs[1]
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("lowers AddListInt");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(
            ir.contains("call i64 @lkrt_lklist_i64_at(ptr"),
            "in-range element folds to at: {ir}"
        );
        assert!(ir.contains("add i64"), "{ir}");
    }

    /// `let m = {"a": 7}; return m["a"];` — a constant `Map<str,i64>` materializes a
    /// map handle (new + set), and `GetFieldK` lowers to the by-value Maybe lookup;
    /// the constant key is interned as a single shared global.
    #[test]
    fn lowers_str_map_const_and_get() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: Vec::new(),
            floats: Vec::new(),
            strings: vec!["a".to_string()],
            heap_values: vec![ConstHeapValueData::Map(vec![(
                RuntimeMapKeyData::ShortStr("a".to_string()),
                ConstRuntimeValueData::Int(7),
            )])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = {"a":7}
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = m
                Instr::abc(Opcode::GetFieldK, 1, 0, 0).raw(),  // r1 = m["a"] (key strings[0])
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            2,
        );
        let mir = lower(&art).expect("map const + get lowers");
        assert_eq!(mir.globals, vec!["a".to_string()], "key interned once");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call ptr @lkrt_lkmap_str_i64_new()"), "{ir}");
        assert!(ir.contains("call void @lkrt_lkmap_str_i64_set(ptr"), "{ir}");
        assert!(
            ir.contains("call { i64, i64 } @lkrt_lkmap_str_i64_get_pair(ptr"),
            "{ir}"
        );
    }

    /// `if (m[k] == nil)` via `BrNotNil` on a map lookup: the `Maybe`'s present bit
    /// drives the branch (`extractvalue … 1` → `icmp ne`).
    #[test]
    fn lowers_nil_branch_on_maybe() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![9, 1, 0],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::Map(vec![(
                RuntimeMapKeyData::Int(1),
                ConstRuntimeValueData::Int(10),
            )])],
        };
        // r0 = {1:10}; r1 = m[9] (Maybe, missing); if (r1 != nil) goto else(pc7) else then(pc5)
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = {1:10}
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = m
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 9 (key)
                Instr::abc(Opcode::GetIndex, 1, 0, 1).raw(),   // r1 = m[9] (Maybe)
                Instr::as_bx(Opcode::BrNotNil, 1, 2).raw(),    // sbx=2 -> jump to pc7 when not-nil
                Instr::abx(Opcode::LoadInt, 2, 1).raw(),       // pc5: r2 = 1 (then: is nil)
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
                Instr::abx(Opcode::LoadInt, 2, 2).raw(), // pc7: r2 = 0 (else)
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("nil-branch on maybe lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("extractvalue { i64, i64 }"), "reads present bit: {ir}");
        assert!(ir.contains("br i1"), "conditional branch: {ir}");
    }

    /// `if (x % 4 == 0) { return 1 } else { return 0 }` via the fused
    /// `BrModNeZeroIntI4` divisibility branch (guarded modulo + compare-to-zero).
    #[test]
    fn lowers_fused_mod_zero_branch() {
        // pc0: r0 = 12
        // pc1: BrModNeZeroIntI4 r0 % 4 != 0, offset=2 (jump to else pc4 when != 0)
        // pc2: r1 = 1 ; pc3: return r1   (then: divisible)
        // pc4: r1 = 0 ; pc5: return r1   (else)
        let art = artifact(
            ints(vec![12, 1, 0]),
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                Instr::branch_i4(Opcode::BrModNeZeroIntI4, 0, 4, 2).raw(),
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                Instr::abx(Opcode::LoadInt, 1, 2).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            2,
        );
        let mir = lower(&art).expect("fused mod-zero branch lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("@lkrt_i64_mod_checked"), "guarded modulo: {ir}");
        assert!(ir.contains("icmp ne i64"), "compare to zero: {ir}");
        assert!(ir.contains("br i1"), "conditional branch: {ir}");
    }

    /// `if (x == 3) { return 100 } else { return 0 }` via the fused `BrNeIntI4`
    /// branch: the single-instruction compare-and-branch lowers like a `CondBr`.
    #[test]
    fn lowers_fused_ne_immediate_branch() {
        // pc0: r0 = 3
        // pc1: BrNeIntI4 r0 != 3, offset=2  (jump to else pc4 when !=)
        // pc2: r1 = 100  (then, r0 == 3)
        // pc3: return r1
        // pc4: r1 = 0    (else)
        // pc5: return r1
        let art = artifact(
            ints(vec![3, 100, 0]),
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                Instr::branch_i4(Opcode::BrNeIntI4, 0, 3, 2).raw(),
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                Instr::abx(Opcode::LoadInt, 1, 2).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            2,
        );
        let mir = lower(&art).expect("fused ne-branch lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("icmp ne i64"), "fused != immediate compare: {ir}");
        assert!(ir.contains("br i1"), "conditional branch: {ir}");
    }

    /// A returned `f64` prints via `lkrt_f64_to_str` (Rust `to_string`, the VM's exact
    /// float display) rather than `printf %g` — whose fixed precision diverges from
    /// the VM's shortest round-trip (e.g. `1.0/7.0`).
    #[test]
    fn float_return_uses_display_helper() {
        let art = artifact(
            floats(vec![1.5, 2.5]),
            vec![
                Instr::abx(Opcode::LoadFloat, 0, 0).raw(),
                Instr::abx(Opcode::LoadFloat, 1, 1).raw(),
                Instr::abc(Opcode::AddFloat, 2, 0, 1).raw(),
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("float return lowers");
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(
            ir.contains("call ptr @lkrt_f64_to_str(double"),
            "float return uses display helper: {ir}"
        );
        assert!(!ir.contains("@lk_f64_fmt, double"), "not the %g path: {ir}");
    }

    /// `"n=${n}"` — numeric interpolation lowers `ConcatString` with an `I64` operand
    /// display-converted via `str.from_i64`.
    #[test]
    fn lowers_concat_string_int_display() {
        let consts = ConstPoolData {
            ints: vec![5],
            floats: Vec::new(),
            strings: vec!["n=".to_string()],
            heap_values: Vec::new(),
        };
        // r0 = 5; r1 = "n="; ConcatString dst=2 b=1 c=0 → "n=" ++ display(5)
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                Instr::abx(Opcode::LoadString, 1, 0).raw(),
                Instr::abc(Opcode::ConcatString, 2, 1, 0).raw(),
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("ConcatString with int display lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        // The int suffix fuses into a single concat_i64 call — no intermediate
        // display string is materialized (or freed).
        assert!(ir.contains("call ptr @lkrt_str_concat_i64(ptr"), "fused concat: {ir}");
        assert!(!ir.contains("call ptr @lkrt_i64_to_str(i64"), "no suffix temp: {ir}");
    }

    /// `"${a}-${b}"` — string interpolation of string vars lowers `ConcatN` to a
    /// chain of `str_concat`.
    #[test]
    fn lowers_concat_n_strings() {
        let consts = ConstPoolData {
            ints: Vec::new(),
            floats: Vec::new(),
            strings: vec!["a".to_string(), "-".to_string(), "b".to_string()],
            heap_values: Vec::new(),
        };
        // r0="a", r1="-", r2="b"; ConcatN dst=3 start=0 count=3 → "a-b"
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadString, 0, 0).raw(),
                Instr::abx(Opcode::LoadString, 1, 1).raw(),
                Instr::abx(Opcode::LoadString, 2, 2).raw(),
                Instr::abc(Opcode::ConcatN, 3, 0, 3).raw(),
                Instr::abc(Opcode::Return1, 3, 0, 0).raw(),
            ],
            4,
        );
        let mir = lower(&art).expect("ConcatN of strings lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        // 3 elements → 2 chained concats.
        assert_eq!(ir.matches("call ptr @lkrt_str_concat(ptr").count(), 2, "{ir}");
    }

    /// `a + b` on two strings is concatenation (the generic `AddInt` opcode) →
    /// `lkrt_str_concat`, yielding a `Str`.
    #[test]
    fn lowers_string_concat() {
        let consts = ConstPoolData {
            ints: Vec::new(),
            floats: Vec::new(),
            strings: vec!["foo".to_string(), "bar".to_string()],
            heap_values: Vec::new(),
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadString, 0, 0).raw(), // r0 = "foo"
                Instr::abx(Opcode::LoadString, 1, 1).raw(), // r1 = "bar"
                Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),  // r2 = r0 + r1
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("string concat lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call ptr @lkrt_str_concat(ptr"), "{ir}");
        // The concat result is a Str, printed via %s on return.
        assert!(ir.contains("@printf(ptr @lk_str_fmt, ptr"), "{ir}");
    }

    /// `"hi" == "hi"` — string equality via the generic `CmpInt` opcode on two `Str`
    /// operands lowers to `str_cmp` compared against 0.
    #[test]
    fn lowers_string_equality() {
        let consts = ConstPoolData {
            ints: Vec::new(),
            floats: Vec::new(),
            strings: vec!["hi".to_string()],
            heap_values: Vec::new(),
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadString, 0, 0).raw(), // r0 = "hi"
                Instr::abx(Opcode::LoadString, 1, 0).raw(), // r1 = "hi"
                Instr::abc(Opcode::CmpInt, 2, 0, 1).raw(),  // r2 = (r0 == r1)
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("string equality lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call i64 @lkrt_str_cmp(ptr"), "{ir}");
        assert!(ir.contains("icmp eq i64"), "compare str_cmp result to 0: {ir}");
    }

    /// `!(x > 3)` — logical `Not` on a `Bool` lowers to `xor i1 …, true`.
    #[test]
    fn lowers_logical_not() {
        let art = artifact(
            ints(vec![5, 3]),
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),     // r0 = 5
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),     // r1 = 3
                Instr::abc(Opcode::CmpGtInt, 2, 0, 1).raw(), // r2 = (5 > 3)
                Instr::abc(Opcode::Not, 3, 2, 0).raw(),      // r3 = !r2
                Instr::abc(Opcode::Return1, 3, 0, 0).raw(),
            ],
            4,
        );
        let mir = lower(&art).expect("logical not lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("xor i1"), "{ir}");
    }

    /// `DivFloat` (and the other float ops) coerce an `I64` operand to `F64` (the VM
    /// does this too, e.g. an `I64` parameter in `x / 2.0`): `10 / 2.0 => 5.0`.
    #[test]
    fn float_arith_coerces_int_operand() {
        let art = artifact(
            ConstPoolData {
                ints: vec![10],
                floats: vec![2.0],
                strings: Vec::new(),
                heap_values: Vec::new(),
            },
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),     // r0 = 10 (i64)
                Instr::abx(Opcode::LoadFloat, 1, 0).raw(),   // r1 = 2.0 (f64)
                Instr::abc(Opcode::DivFloat, 2, 0, 1).raw(), // r2 = r0 / r1
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("float div with int operand lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("sitofp i64"), "int operand widened to double: {ir}");
        assert!(ir.contains("@lkrt_f64_div_checked(double"), "{ir}");
    }

    /// An empty `{}` used with an int-index store is typed int-keyed by lookahead:
    /// `let m = {}; m[5] = 50; return m[5];` lowers via the `i64_i64` map handle.
    #[test]
    fn empty_map_int_key_lookahead() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![5, 50],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::Map(Vec::new())], // {}
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = {}
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = m
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 5 (key)
                Instr::abx(Opcode::LoadInt, 2, 1).raw(),       // r2 = 50 (value)
                Instr::abc(Opcode::SetIndex, 0, 1, 2).raw(),   // m[5] = 50
                Instr::abx(Opcode::LoadInt, 2, 0).raw(),       // r2 = 5 (key)
                Instr::abc(Opcode::GetIndex, 1, 0, 2).raw(),   // r1 = m[5]
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("empty int-key map lowers via lookahead");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(
            ir.contains("call ptr @lkrt_lkmap_i64_i64_new()"),
            "empty {{}} inferred int-keyed: {ir}"
        );
        assert!(ir.contains("call void @lkrt_lkmap_i64_i64_set(ptr"), "{ir}");
        // `str_i64` symbols appear in the prelude declarations, but no string-keyed
        // map is *called* here.
        assert!(
            !ir.contains("call ptr @lkrt_lkmap_str_i64_new()"),
            "not string-keyed: {ir}"
        );
    }

    /// `let m = {1: 1.5}; return m[1];` — a constant int-keyed f64-valued map
    /// materializes an `i64→f64` handle; `GetIndex` yields a `MaybeF64`.
    #[test]
    fn lowers_int_f64_map() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![1],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::Map(vec![(
                RuntimeMapKeyData::Int(1),
                ConstRuntimeValueData::Float(1.5),
            )])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),
                Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                Instr::abc(Opcode::GetIndex, 1, 0, 2).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("int-f64 map lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call ptr @lkrt_lkmap_i64_f64_new()"), "{ir}");
        assert!(
            ir.contains("call { double, i64 } @lkrt_lkmap_i64_f64_get_pair(ptr"),
            "{ir}"
        );
    }

    /// `let m = {"a": 1.5}; return m["a"];` — a constant str-keyed f64-valued map
    /// materializes an `str→f64` handle; `GetFieldK` yields a `MaybeF64`.
    #[test]
    fn lowers_str_f64_map() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: Vec::new(),
            floats: Vec::new(),
            strings: vec!["a".to_string()],
            heap_values: vec![ConstHeapValueData::Map(vec![(
                RuntimeMapKeyData::ShortStr("a".to_string()),
                ConstRuntimeValueData::Float(1.5),
            )])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),
                Instr::abc(Opcode::GetFieldK, 1, 0, 0).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            2,
        );
        let mir = lower(&art).expect("str-f64 map lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call ptr @lkrt_lkmap_str_f64_new()"), "{ir}");
        assert!(ir.contains("call void @lkrt_lkmap_str_f64_set(ptr"), "{ir}");
        assert!(
            ir.contains("call { double, i64 } @lkrt_lkmap_str_f64_get_pair(ptr"),
            "{ir}"
        );
    }

    /// `let m = {1:10, 2:20}; return m[2];` — a constant int-keyed map materializes an
    /// `i64→i64` handle (new + set), and `GetIndex` lowers to the by-value Maybe lookup.
    #[test]
    fn lowers_int_key_map() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![2],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::Map(vec![
                (RuntimeMapKeyData::Int(1), ConstRuntimeValueData::Int(10)),
                (RuntimeMapKeyData::Int(2), ConstRuntimeValueData::Int(20)),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = {1:10, 2:20}
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = m
                Instr::abx(Opcode::LoadInt, 2, 0).raw(),       // r2 = 2 (key)
                Instr::abc(Opcode::GetIndex, 1, 0, 2).raw(),   // r1 = m[2]
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("int-key map lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call ptr @lkrt_lkmap_i64_i64_new()"), "{ir}");
        assert!(ir.contains("call void @lkrt_lkmap_i64_i64_set(ptr"), "{ir}");
        assert!(
            ir.contains("call { i64, i64 } @lkrt_lkmap_i64_i64_get_pair(ptr"),
            "{ir}"
        );
    }

    /// A returned string literal materializes an interned global and prints via the
    /// entry's `%s` path.
    #[test]
    fn lowers_string_constant_return() {
        let consts = ConstPoolData {
            ints: Vec::new(),
            floats: Vec::new(),
            strings: vec!["hello".to_string()],
            heap_values: Vec::new(),
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadString, 0, 0).raw(), // r0 = "hello"
                Instr::abc(Opcode::Return1, 0, 0, 0).raw(),
            ],
            1,
        );
        let mir = lower(&art).expect("string constant lowers");
        assert_eq!(mir.globals, vec!["hello".to_string()]);
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("@lk_str_0"), "materializes a string global: {ir}");
        assert!(ir.contains("@printf(ptr @lk_str_fmt, ptr"), "prints via %s: {ir}");
    }

    /// Identical string constants intern to a single shared global.
    #[test]
    fn interns_duplicate_strings() {
        let mut globals = vec![];
        let a = intern_global(&mut globals, "k");
        let b = intern_global(&mut globals, "k");
        let c = intern_global(&mut globals, "other");
        assert_eq!((a, b, c), (0, 0, 1));
        assert_eq!(globals, vec!["k".to_string(), "other".to_string()]);
    }

    /// A dead `LoadString` (common in loop setup) must not block lowering: the
    /// register is left undefined and the surrounding integer code still lowers.
    #[test]
    fn dead_string_load_does_not_block_lowering() {
        let consts = ConstPoolData {
            ints: vec![42],
            floats: Vec::new(),
            strings: vec!["unused".to_string()],
            heap_values: Vec::new(),
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),    // r0 = 42
                Instr::abx(Opcode::LoadString, 1, 0).raw(), // r1 = "unused" (dead)
                Instr::abc(Opcode::Return1, 0, 0, 0).raw(), // return r0
            ],
            2,
        );
        let mir = lower(&art).expect("dead string load lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("ret i64 42") || ir.contains(" 42"), "{ir}");
    }

    /// An out-of-range constant index rejects (falls back) — never risks the VM's
    /// out-of-range → nil semantics being miscompiled.
    /// An out-of-range index (even a constant one) is no longer rejected: it takes
    /// the dynamic `Maybe<Int>` path and returns `nil`, matching the VM. Codegen
    /// emits the by-value `get_pair` call and the nil-or-value return branch.
    #[test]
    fn out_of_range_index_lowers_to_maybe_returning_nil() {
        use lk_core::vm::ConstHeapValueData;
        let consts = ConstPoolData {
            ints: vec![5],
            floats: Vec::new(),
            strings: Vec::new(),
            heap_values: vec![ConstHeapValueData::List(vec![
                ConstRuntimeValueData::Int(1),
                ConstRuntimeValueData::Int(2),
            ])],
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
                Instr::abc(Opcode::Move, 0, 1, 0).raw(),
                Instr::abx(Opcode::LoadInt, 2, 0).raw(), // index 5, out of range
                Instr::abc(Opcode::GetList, 1, 0, 2).raw(),
                Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("out-of-range index lowers via Maybe");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call { i64, i64 } @lkrt_lklist_i64_get_pair(ptr"), "{ir}");
        // Present branch prints the element; absent branch prints nothing (just
        // the arena cleanup + `ret`), matching the VM's silent top-level nil return.
        assert!(ir.contains("extractvalue { i64, i64 }"), "{ir}");
        assert!(
            ir.contains("none:\n  call void @lkrt_cleanup()\n  ret i32 0"),
            "absent branch prints nothing: {ir}"
        );
    }

    #[test]
    fn lowers_straightline_integer_division() {
        let art = artifact(
            ints(vec![20, 4]),
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),
                Instr::abc(Opcode::DivInt, 2, 0, 1).raw(),
                Instr::abc(Opcode::Return, 2, 1, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("call i64 @lkrt_i64_div_checked"));
        assert!(!ir.contains("sdiv"));
    }

    #[test]
    fn lowers_early_return_conditional() {
        let art = artifact(
            ints(vec![3, 5]),
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),
                Instr::abc(Opcode::CmpLtInt, 2, 0, 1).raw(),
                Instr::as_bx(Opcode::BrFalse, 2, 1).raw(),
                Instr::abc(Opcode::Return, 0, 1, 0).raw(),
                Instr::abc(Opcode::Return, 1, 1, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert!(matches!(mir.functions[0].blocks[0].term, Term::CondBr { .. }));
    }

    #[test]
    fn lowers_if_else_merge_with_phi() {
        let art = artifact(
            ints(vec![3, 5]),
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),
                Instr::abc(Opcode::CmpLtInt, 2, 0, 1).raw(),
                Instr::as_bx(Opcode::BrFalse, 2, 2).raw(),
                Instr::abc(Opcode::Move, 3, 0, 0).raw(),
                Instr::sj(Opcode::Jmp, 1).raw(),
                Instr::abc(Opcode::Move, 3, 1, 0).raw(),
                Instr::abc(Opcode::Return, 3, 1, 0).raw(),
            ],
            4,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let merge = mir.functions[0]
            .blocks
            .iter()
            .find(|b| matches!(b.term, Term::Ret(Some(_))))
            .unwrap();
        assert_eq!(merge.params.len(), 1, "join block carries one phi param for r3");
        assert!(lk_aot_codegen::render_module(&mir).contains("phi i64 ["));
    }

    #[test]
    fn lowers_fused_compare_branch() {
        let art = artifact(
            ints(vec![3, 99]),
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                Instr::abc(Opcode::TestLeIntI, 0, 0, 5).raw(),
                Instr::sj(Opcode::Jmp, 1).raw(),
                Instr::abc(Opcode::Return, 0, 1, 0).raw(),
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),
                Instr::abc(Opcode::Return, 1, 1, 0).raw(),
            ],
            2,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("icmp sle i64"), "{ir}");
        assert!(ir.contains("br i1 "));
    }

    /// `s=0; i=1; while (i <= 5) { s += i; i += 1; } return s;` — a real loop with a
    /// back-edge, exercising Braun loop-header phi construction. Sum 1..=5 = 15.
    #[test]
    fn lowers_counted_loop_with_backedge() {
        let art = artifact(
            ints(vec![0, 1]),
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),       // 0: s=0
                Instr::abx(Opcode::LoadInt, 1, 1).raw(),       // 1: i=1
                Instr::abc(Opcode::TestLeIntI, 1, 0, 5).raw(), // 2: test i<=5 (jump when false)
                Instr::sj(Opcode::Jmp, 3).raw(),               // 3: (fused) -> pc7 (exit)
                Instr::abc(Opcode::AddInt, 0, 0, 1).raw(),     // 4: s += i
                Instr::abc(Opcode::AddIntI, 1, 1, 1).raw(),    // 5: i += 1
                Instr::sj(Opcode::Jmp, -5).raw(),              // 6: -> pc2 (back-edge)
                Instr::abc(Opcode::Return, 0, 1, 0).raw(),     // 7: return s
            ],
            2,
        );
        let mir = lower(&art).expect("lowers loop");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        // The loop header (block containing the fused test) carries phi params for
        // the loop-carried s and i.
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("phi i64 ["), "loop header needs phis: {ir}");
    }

    #[test]
    fn lowers_float_arithmetic() {
        let art = artifact(
            floats(vec![1.5, 2.5]),
            vec![
                Instr::abx(Opcode::LoadFloat, 0, 0).raw(),
                Instr::abx(Opcode::LoadFloat, 1, 1).raw(),
                Instr::abc(Opcode::AddFloat, 2, 0, 1).raw(),
                Instr::abc(Opcode::Return, 2, 1, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert_eq!(mir.functions[0].ret, Ty::F64);
        assert!(lk_aot_codegen::render_module(&mir).contains("fadd double"));
    }

    #[test]
    fn int_arith_dispatches_to_float_on_float_operands() {
        // `AddInt` dispatches on runtime operand type: two floats → float add.
        let art = artifact(
            floats(vec![1.5, 2.5]),
            vec![
                Instr::abx(Opcode::LoadFloat, 0, 0).raw(),
                Instr::abx(Opcode::LoadFloat, 1, 1).raw(),
                Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),
                Instr::abc(Opcode::Return, 2, 1, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
        assert_eq!(mir.functions[0].ret, Ty::F64);
        assert!(lk_aot_codegen::render_module(&mir).contains("fadd double"));
    }

    #[test]
    fn int_add_coerces_mixed_operands() {
        // int + float → the int operand is widened (`sitofp`) then float-added.
        let consts = ConstPoolData {
            ints: vec![10],
            floats: vec![2.5],
            strings: Vec::new(),
            heap_values: Vec::new(),
        };
        let art = artifact(
            consts,
            vec![
                Instr::abx(Opcode::LoadInt, 0, 0).raw(),
                Instr::abx(Opcode::LoadFloat, 1, 0).raw(),
                Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),
                Instr::abc(Opcode::Return, 2, 1, 0).raw(),
            ],
            3,
        );
        let mir = lower(&art).expect("lowers");
        assert_eq!(mir.functions[0].ret, Ty::F64);
        let ir = lk_aot_codegen::render_module(&mir);
        assert!(ir.contains("sitofp i64"), "{ir}");
        assert!(ir.contains("fadd double"), "{ir}");
    }
}
