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
    Ty, ValueId,
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
    /// A register was read with no reaching definition on any predecessor path.
    UndefinedOperand {
        pc: usize,
        reg: u8,
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
}

/// Global names treated as stdlib module objects when read via `GetGlobal`.
/// Only modules with at least one [`module_call_abi`] mapping belong here.
const MODULE_GLOBALS: &[&str] = &["os", "time", "env", "math"];

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
    /// Diagnostic names for the mutable-global table (slot-indexed).
    global_names: Vec<String>,
    /// Final compact `slot → gvar` numbering, built once signatures converge
    /// (empty during the fixpoint passes, whose emitted MIR is discarded).
    gvar_of: std::collections::HashMap<u16, u32>,
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
    let module = &artifact.module;
    if module.functions.is_empty() {
        return Err(Unsupported::NoEntry);
    }
    let n = module.functions.len();

    // Reachability from the entry via `CallDirect`. Functions that are defined but
    // never directly called (e.g. a small helper the front end inlined at every use)
    // are dead for AOT; lowering them would pointlessly fail the whole module if they
    // use a shape we don't support, so we skip them entirely.
    let reachable = reachable_functions(module);

    let global_count = module.globals.len();
    let mut sig = SigInfer {
        param_obs: module
            .functions
            .iter()
            .map(|f| vec![None; f.param_count as usize])
            .collect(),
        ret_types: vec![Ty::I64; n],
        conflict: false,
        global_tys: vec![None; global_count],
        initialized_globals: prescan_initialized_globals(module, global_count),
        global_names: module.globals.clone(),
        gvar_of: std::collections::HashMap::new(),
    };

    // Fixpoint: re-lower every function, refining inferred parameter/return types
    // (bounded — the scalar lattice converges quickly). Transient failures are
    // tolerated here (a function may not lower until the types it depends on have
    // converged); the final pass below is authoritative and propagates errors.
    for _ in 0..2 * n + 2 {
        let snapshot = (sig.param_obs.clone(), sig.ret_types.clone());
        for (fi, func) in module.functions.iter().enumerate() {
            if !reachable[fi] {
                continue;
            }
            let is_entry = fi as u32 == module.entry;
            let mut scratch = Vec::new();
            if let Ok(mf) = lower_function(
                func,
                &module.functions,
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
        if sig.conflict {
            return Err(Unsupported::ReturnTypeConflict);
        }
        if snapshot == (sig.param_obs.clone(), sig.ret_types.clone()) {
            break;
        }
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
    // whole module (caller falls back); `FuncId`s keep their original module indices.
    let mut globals: Vec<String> = Vec::new();
    let mut functions = Vec::with_capacity(n);
    for (fi, func) in module.functions.iter().enumerate() {
        if !reachable[fi] {
            continue;
        }
        let is_entry = fi as u32 == module.entry;
        functions.push(lower_function(
            func,
            &module.functions,
            fi as u32,
            module.entry,
            is_entry,
            &mut globals,
            &module.globals,
            &mut sig,
        )?);
    }
    if sig.conflict {
        return Err(Unsupported::ReturnTypeConflict);
    }
    Ok(MirModule {
        abi_version: lk_aot_abi::ABI_VERSION,
        globals,
        mutable_globals,
        entry: FuncId(module.entry),
        functions,
    })
}

/// Slots the entry function writes before any control flow or user-function
/// call: the linear instruction prefix up to the first branch/jump/return/
/// `CallDirect`/`CallNamed`. Reads of other globals could observe the VM's nil
/// initialization (native storage zero-initializes instead), so only these
/// slots are readable via `GetGlobal`. Runtime-builtin `Call`s (println,
/// os.clock, …) cannot read user globals and do not stop the scan.
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
            if let Ok(instr) = Instr::try_from_raw(*raw)
                && instr.opcode() == Opcode::CallDirect
            {
                let callee = instr.b() as usize;
                if callee < n && !reachable[callee] {
                    reachable[callee] = true;
                    stack.push(callee);
                }
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
    if func.capture_count != 0 {
        return Err(Unsupported::EntryHasCaptures(func.capture_count));
    }
    if is_entry && func.param_count != 0 {
        return Err(Unsupported::EntryHasParams(func.param_count));
    }
    let param_count = func.param_count as usize;

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
    let mut ssa = Ssa::new(reg_count, preds, total_blocks);
    // Function parameters occupy r0..r(param_count-1) at entry; each takes its
    // inferred type (the argument type observed at call sites, `I64` by default).
    // They seed the entry block's register file as its first SSA values.
    let mut fn_params: Vec<(ValueId, Ty)> = Vec::with_capacity(param_count);
    for r in 0..param_count {
        let pty = sig.param_ty(func_index as usize, r);
        let pv = ssa.new_val();
        ssa.current_def[0][r] = Some((pv, pty));
        fn_params.push((pv, pty));
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
                &instrs[pc],
                pc,
            )?;
        }
        // Resolve the terminator's value reads while this block is current.
        match exit {
            Some(Exit::Ret(Some(reg))) => {
                let (v, ty) = ssa.read(reg, bi, start)?;
                match ret_ty {
                    Some(prev) if prev != ty => return Err(Unsupported::ReturnTypeConflict),
                    _ => ret_ty = Some(ty),
                }
                ret_val[bi] = Some(v);
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
                    Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr => {
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
    // User (non-entry) functions return scalars or `Str`/handle pointers
    // (arena-owned until exit). Returning a `Maybe` carrier or `Nil` across the
    // direct-call boundary isn't modelled — reject (fall back).
    if !is_entry && matches!(ret, Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr) {
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
        Some(Exit::Ret(Some(_))) => Term::Ret(Some(ret_val.expect("ret value resolved"))),
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
    /// SSA value → its compile-time string content (`LoadString` /
    /// `LoadHeapConst` long strings), used to expand `println` format strings
    /// at lower time.
    const_strs: std::collections::HashMap<ValueId, String>,
    /// `(block, register)` → global reference (runtime builtin, module object,
    /// or resolved module function) loaded there by `GetGlobal`/`GetIndex` and
    /// propagated by `Move`. Block-local by construction; any write to the
    /// register clears it.
    builtin_regs: std::collections::HashMap<(usize, u8), GlobalRef>,
    /// Per-block trailing instructions added for phi-edge type conversions
    /// (`Maybe` ↔ scalar merges); appended after the block's own instructions
    /// when the MIR blocks are assembled.
    edge_insts: Vec<Vec<Inst>>,
}

impl Ssa {
    fn new(reg_count: usize, preds: Vec<Vec<usize>>, total_blocks: usize) -> Self {
        Self {
            reg_count,
            preds,
            current_def: vec![vec![None; reg_count]; total_blocks],
            sealed: vec![false; total_blocks],
            filled: vec![false; total_blocks],
            phis: (0..total_blocks).map(|_| Vec::new()).collect(),
            incomplete: (0..total_blocks).map(|_| Vec::new()).collect(),
            single_fallthrough_target: vec![None; total_blocks],
            next_val: 0,
            const_int: std::collections::HashMap::new(),
            list_len: std::collections::HashMap::new(),
            const_strs: std::collections::HashMap::new(),
            builtin_regs: std::collections::HashMap::new(),
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
            self.current_def[block][reg as usize] = Some(value);
            self.builtin_regs.remove(&(block, reg));
        }
    }

    fn read(&mut self, reg: u8, block: usize, pc: usize) -> Result<Reg, Unsupported> {
        if let Some(v) = self.current_def[block][reg as usize] {
            return Ok(v);
        }
        self.read_recursive(reg, block, pc)
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
    fn reg_const_str(&self, reg: u8, block: usize) -> Option<String> {
        let mut visited = std::collections::HashSet::new();
        let mut found: Option<String> = None;
        if self.collect_reg_const_str(reg, block, &mut visited, &mut found) {
            found
        } else {
            None
        }
    }

    fn collect_reg_const_str(
        &self,
        reg: u8,
        block: usize,
        visited: &mut std::collections::HashSet<(usize, u8)>,
        found: &mut Option<String>,
    ) -> bool {
        if !visited.insert((block, reg)) {
            return true;
        }
        if let Some((v, ty)) = self.current_def[block][reg as usize] {
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
                    let phi_reg = phi.reg as u8;
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
    fn phi_ty(&mut self, reg: u8, block: usize, pc: usize) -> Result<Ty, Unsupported> {
        let preds = self.preds[block].clone();
        for p in preds {
            if self.filled[p] {
                return Ok(self.read(reg, p, pc)?.1);
            }
        }
        Err(Unsupported::UndefinedOperand { pc, reg })
    }

    fn read_recursive(&mut self, reg: u8, block: usize, pc: usize) -> Result<Reg, Unsupported> {
        let value: Reg = if !self.sealed[block] {
            let ty = self.phi_ty(reg, block, pc)?;
            let param = self.new_val();
            let idx = self.phis[block].len();
            self.phis[block].push(Phi {
                param,
                reg: reg as usize,
                ty,
                operands: Vec::new(),
            });
            self.incomplete[block].push(idx);
            (param, ty)
        } else if self.preds[block].len() == 1 {
            let p = self.preds[block][0];
            self.read(reg, p, pc)?
        } else if self.preds[block].is_empty() {
            return Err(Unsupported::UndefinedOperand { pc, reg });
        } else {
            let ty = self.phi_ty(reg, block, pc)?;
            let param = self.new_val();
            let idx = self.phis[block].len();
            self.phis[block].push(Phi {
                param,
                reg: reg as usize,
                ty,
                operands: Vec::new(),
            });
            // Break cycles before reading operands.
            self.current_def[block][reg as usize] = Some((param, ty));
            self.add_phi_operands(block, idx, pc)?;
            (param, ty)
        };
        self.current_def[block][reg as usize] = Some(value);
        Ok(value)
    }

    fn add_phi_operands(&mut self, block: usize, phi_idx: usize, pc: usize) -> Result<(), Unsupported> {
        let reg = self.phis[block][phi_idx].reg as u8;
        let phi_ty = self.phis[block][phi_idx].ty;
        let preds = self.preds[block].clone();
        for p in preds {
            let (v, ty) = self.read(reg, p, pc)?;
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
            // call-window base before the `Call`.
            if let Some(global_ref) = ssa.builtin_regs.get(&(block, instr.b())).cloned() {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
                return Ok(());
            }
            let src = ssa.read(instr.b(), block, pc)?;
            ssa.write(instr.a(), block, src);
        }
        Opcode::Move2 => {
            // Fused adjacent moves: `a ← b`, then `b ← c`. The VM reads `b`
            // before overwriting it; SSA reads naturally see the old value.
            if let Some(global_ref) = ssa.builtin_regs.get(&(block, instr.b())).cloned() {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
            } else {
                let first = ssa.read(instr.b(), block, pc)?;
                ssa.write(instr.a(), block, first);
            }
            if let Some(global_ref) = ssa.builtin_regs.get(&(block, instr.c())).cloned() {
                ssa.builtin_regs.insert((block, instr.b()), global_ref);
            } else {
                let second = ssa.read(instr.c(), block, pc)?;
                ssa.write(instr.b(), block, second);
            }
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
        Opcode::GetIndexStrI | Opcode::SetIndexStrI => {
            // Composite string-int key access (`m["n${i}"]`): the key is the
            // compiler-proven constant prefix plus the decimal suffix register.
            // Built as `concat(prefix, i64_to_str(suffix))`; the map ABI copies
            // the key, so the fresh temporary frees right after the call.
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
            let suffix_str = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(suffix_str),
                callee: AbiRef::new("str", "from_i64"),
                args: vec![suffix],
            });
            let key = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(key),
                callee: AbiRef::new("str", "concat"),
                args: vec![prefix_v, suffix_str],
            });
            free_owned_str(insts, suffix_str);
            if is_set {
                let (value, value_ty) = ssa.read(instr.c(), block, pc)?;
                let set_fn = match (map_ty, value_ty) {
                    (Ty::MapStrI64, Ty::I64) => "str_i64_set",
                    (Ty::MapStrF64, Ty::F64) => "str_f64_set",
                    _ => return Err(Unsupported::TypeMismatch { pc }),
                };
                insts.push(Inst::Call {
                    dst: None,
                    callee: AbiRef::new("map_h", set_fn),
                    args: vec![handle, key, value],
                });
                free_owned_str(insts, key);
            } else {
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
            let lhs = ssa.read_typed(instr.b(), block, Ty::I64, pc)?;
            let rhs = ssa.read_typed(instr.c(), block, Ty::I64, pc)?;
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
            let lhs = ssa.read_typed(instr.b(), block, Ty::I64, pc)?;
            let rhs = ssa.read_typed(instr.c(), block, Ty::I64, pc)?;
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
            let acc = ssa.read_typed(instr.a(), block, Ty::I64, pc)?;
            let lhs = ssa.read_typed(instr.b(), block, Ty::I64, pc)?;
            let rhs = ssa.read_typed(instr.c(), block, Ty::I64, pc)?;
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
            let acc = ssa.read_typed(instr.a(), block, Ty::I64, pc)?;
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
            let (lv, lty) = read_scalar(ssa, insts, instr.b(), block, pc)?;
            let (rv, rty) = read_scalar(ssa, insts, instr.c(), block, pc)?;
            let (float, lhs, rhs) = match (lty, rty) {
                (Ty::I64, Ty::I64) => (false, lv, rv),
                (Ty::F64, Ty::F64) | (Ty::I64, Ty::F64) | (Ty::F64, Ty::I64) => (
                    true,
                    coerce_to_f64(ssa, insts, lv, lty),
                    coerce_to_f64(ssa, insts, rv, rty),
                ),
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
        Opcode::CallDirect => {
            // Register-window call: `a`=dst register, `b`=callee function index,
            // `c`=argument count; the args occupy registers `[a+1, a+1+c)`. Each
            // argument's observed scalar type refines the callee's parameter type
            // (`sig.param_obs`); disagreeing sites mark the callee polymorphic
            // (`sig.conflict` → whole-module fallback). The result takes the callee's
            // inferred return type, so `f64`/`bool`-returning calls type correctly.
            let callee_idx = instr.b() as usize;
            if callee_idx >= funcs.len() || callee_idx == entry as usize {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            }
            let argc = instr.c() as usize;
            if argc != funcs[callee_idx].param_count as usize {
                return Err(Unsupported::Opcode { pc, op: instr.opcode() });
            }
            let dst_reg = instr.a();
            let mut args = Vec::with_capacity(argc);
            for i in 0..argc {
                let arg_reg = dst_reg.wrapping_add(1).wrapping_add(i as u8);
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
            let dst = ssa.new_val();
            insts.push(Inst::CallFn {
                dst: Some(dst),
                func: FuncId(callee_idx as u32),
                args,
            });
            let ret = sig.ret_types.get(callee_idx).copied().unwrap_or(Ty::I64);
            ssa.write(dst_reg, block, (dst, ret));
        }
        // Direct calls address the callee by index, so the loaded function
        // value itself only flows into the compiler's global-table storage
        // (`SetGlobal`), which stays a no-op.
        Opcode::LoadFunction => {
            ssa.builtin_regs.insert((block, instr.a()), GlobalRef::UserFn);
        }
        Opcode::SetGlobal => {
            // Storing a function value into the global table is the compiler's
            // top-level `fn` bookkeeping — a no-op natively.
            if let Some(GlobalRef::UserFn) = ssa.builtin_regs.get(&(block, instr.a())) {
                return Ok(());
            }
            // Writing a global whose *name* this lowering recognizes would let
            // later `GetGlobal` reads resolve to the stale builtin/module
            // meaning and miscompile (`println = f; println(x)`), so those
            // writes reject the program.
            let slot = instr.bx();
            let name = module_globals.get(slot as usize).map(String::as_str);
            if let Some(name) = name
                && (matches!(name, "println" | "print" | "assert") || MODULE_GLOBALS.contains(&name))
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
                Some(name) if MODULE_GLOBALS.contains(&name) => Some(GlobalRef::Module(name.to_string())),
                _ => None,
            };
            if let Some(global_ref) = global_ref {
                ssa.builtin_regs.insert((block, instr.a()), global_ref);
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
            match ssa.builtin_regs.get(&(block, base)).cloned() {
                Some(GlobalRef::Builtin(builtin)) => {
                    lower_builtin_call(ssa, insts, globals, builtin, base, instr.c() as usize, block, pc)?;
                }
                Some(GlobalRef::ModuleFn(module, name)) => {
                    lower_module_call(ssa, insts, &module, &name, base, instr.c() as usize, block, pc)?;
                }
                Some(GlobalRef::Module(_)) | Some(GlobalRef::UserFn) | None => {
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
            let (s, _fresh) = to_display_str(ssa, insts, globals, v, ty, pc)?;
            ssa.write(instr.a(), block, (s, Ty::Str));
        }
        Opcode::ConcatString => {
            // `a` = dst, `b` = lhs, `c` = rhs. Concatenate `display(lhs) ++
            // display(rhs)` (each operand display-converted, as the VM does).
            let (lv, lty) = ssa.read(instr.b(), block, pc)?;
            let (rv, rty) = ssa.read(instr.c(), block, pc)?;
            let (l, l_fresh) = to_display_str(ssa, insts, globals, lv, lty, pc)?;
            let (r, r_fresh) = to_display_str(ssa, insts, globals, rv, rty, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("str", "concat"),
                args: vec![l, r],
            });
            // Display temporaries are dead once concatenated — free them.
            if l_fresh {
                free_owned_str(insts, l);
            }
            if r_fresh {
                free_owned_str(insts, r);
            }
            ssa.write(instr.a(), block, (dst, Ty::Str));
        }
        Opcode::ConcatN => {
            // `a` = dst, `b` = first element register, `c` = element count. The VM
            // display-converts each element then concatenates; each element is
            // display-converted (`Str`/`Int`/`Bool`) and folded via repeated
            // `str_concat`. A float/other element rejects (falls back).
            let start = instr.b();
            let count = instr.c() as usize;
            let mut vals = Vec::with_capacity(count);
            for i in 0..count {
                let (v, ty) = ssa.read(start.wrapping_add(i as u8), block, pc)?;
                vals.push(to_display_str(ssa, insts, globals, v, ty, pc)?);
            }
            let result = if let Some((&(first, first_fresh), rest)) = vals.split_first() {
                let mut acc = first;
                let mut acc_fresh = first_fresh;
                for &(v, fresh) in rest {
                    let dst = ssa.new_val();
                    insts.push(Inst::Call {
                        dst: Some(dst),
                        callee: AbiRef::new("str", "concat"),
                        args: vec![acc, v],
                    });
                    // The consumed accumulator and element temporary are dead;
                    // free the ones this lowering allocated.
                    if acc_fresh {
                        free_owned_str(insts, acc);
                    }
                    if fresh {
                        free_owned_str(insts, v);
                    }
                    acc = dst;
                    acc_fresh = true;
                }
                acc
            } else {
                // Empty concat → the empty string.
                let gid = intern_global(globals, "");
                let dst = ssa.new_val();
                insts.push(Inst::Const {
                    dst,
                    value: Const::Str(GlobalId(gid)),
                });
                dst
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
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
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
                    ssa.write(instr.a(), block, (handle, list_ty));
                }
                ConstHeapValueData::Map(entries) => {
                    // `Map<str,i64>` / `Map<i64,i64>` / `Map<str,f64>` — keys uniformly
                    // string or int, values uniformly int or float. Other shapes (mixed,
                    // int-key+f64-value, non-scalar values) fall back.
                    let all_int_vals = entries.iter().all(|(_, v)| matches!(v, ConstRuntimeValueData::Int(_)));
                    let all_f64_vals = entries
                        .iter()
                        .all(|(_, v)| matches!(v, ConstRuntimeValueData::Float(_)));
                    let all_str_keys = entries
                        .iter()
                        .all(|(k, _)| matches!(k, RuntimeMapKeyData::ShortStr(_) | RuntimeMapKeyData::String(_)));
                    let all_int_keys = entries.iter().all(|(k, _)| matches!(k, RuntimeMapKeyData::Int(_)));
                    if !(all_int_vals || all_f64_vals) || !(all_str_keys || all_int_keys) {
                        return Err(Unsupported::Opcode { pc, op: instr.opcode() });
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
                _ => return Err(Unsupported::Opcode { pc, op: instr.opcode() }),
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
            // constant string key); the register carries the ref, not a value.
            if let Some(GlobalRef::Module(module)) = ssa.builtin_regs.get(&(block, instr.b())).cloned() {
                let name = {
                    let key = ssa.read(instr.c(), block, pc).ok().map(|(v, _)| v);
                    key.and_then(|v| ssa.const_strs.get(&v).cloned())
                        .or_else(|| ssa.reg_const_str(instr.c(), block))
                };
                let Some(name) = name else {
                    return Err(Unsupported::Opcode { pc, op: instr.opcode() });
                };
                ssa.builtin_regs
                    .insert((block, instr.a()), GlobalRef::ModuleFn(module, name));
                return Ok(());
            }
            // `a` = dst, `b` = container register, `c` = key register.
            let (handle, list_ty) = ssa.read(instr.b(), block, pc)?;
            // String-keyed map reads take a `Str` key (dynamic template keys
            // included); a missing key is the `Maybe` nil model.
            if matches!(list_ty, Ty::MapStrI64 | Ty::MapStrF64) {
                let key = ssa.read_typed(instr.c(), block, Ty::Str, pc)?;
                let dst = ssa.new_val();
                let maybe_ty = if list_ty == Ty::MapStrI64 {
                    insts.push(Inst::MapGetMaybe { dst, handle, key });
                    Ty::MaybeI64
                } else {
                    insts.push(Inst::MapGetMaybeStrF64 { dst, handle, key });
                    Ty::MaybeF64
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
            // `a` = dst (bool), `b` = needle, `c` = haystack. Only list haystacks
            // (with a matching element type) are lowered; string/map/set haystacks
            // fall back. The runtime helper returns 0/1, narrowed here to an `i1`.
            let (handle, list_ty) = ssa.read(instr.c(), block, pc)?;
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
        op @ (Opcode::AddListInt | Opcode::SubListInt) => {
            // Fused `acc ±= list[index]` (a = accumulator/dst, b = list, c = index).
            // The VM reads the element with `read_known_int_list_index` (halts on an
            // out-of-range index — *not* nil), so the element read unwraps with a
            // present-assert, matching that halt; in a `for`/`while i<len` loop the
            // index is always in range so the assert never fires.
            let acc = ssa.read_typed(instr.a(), block, Ty::I64, pc)?;
            let elem = list_i64_element_scalar(ssa, insts, instr.b(), instr.c(), block, pc)?;
            let dst = ssa.new_val();
            insts.push(Inst::IntBin {
                dst,
                op: if matches!(op, Opcode::AddListInt) {
                    IntBinOp::Add
                } else {
                    IntBinOp::Sub
                },
                lhs: acc,
                rhs: elem,
            });
            ssa.write(instr.a(), block, (dst, Ty::I64));
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
fn to_display_str(
    ssa: &mut Ssa,
    insts: &mut Vec<Inst>,
    globals: &mut Vec<String>,
    v: ValueId,
    ty: Ty,
    pc: usize,
) -> Result<(ValueId, bool), Unsupported> {
    match ty {
        Ty::Str => Ok((v, false)),
        // A `Maybe` displays its value when present and `nil` when absent
        // (matching the VM's display of a missing-key read). The value-side
        // conversion runs unconditionally (its result is arena-owned and
        // simply unused on the absent path), then a select picks the text.
        Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr => {
            let raw = ssa.new_val();
            insts.push(Inst::MaybeValue {
                dst: raw,
                src: v,
                maybe_ty: ty,
            });
            let scalar_ty = match ty {
                Ty::MaybeI64 => Ty::I64,
                Ty::MaybeF64 => Ty::F64,
                _ => Ty::Str,
            };
            let (value_str, _) = to_display_str(ssa, insts, globals, raw, scalar_ty, pc)?;
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
        _ => Err(Unsupported::TypeMismatch { pc }),
    }
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
                let (msg, fresh) = to_display_str(ssa, insts, globals, mv, mty, pc)?;
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
    // `math.floor` dispatches on the argument's static type, matching the VM's
    // `integer_round`: an `Int` passes through unchanged, a `Float` rounds via
    // the lkrt helper (`floor() as i64`).
    if module == "math" && name == "floor" {
        if argc != 1 {
            return Err(Unsupported::Opcode { pc, op: Opcode::Call });
        }
        let (v, ty) = read_scalar(ssa, insts, base.wrapping_add(1), block, pc)?;
        match ty {
            Ty::I64 => ssa.write(base, block, (v, Ty::I64)),
            Ty::F64 => {
                let dst = ssa.new_val();
                insts.push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("math", "floor"),
                    args: vec![v],
                });
                ssa.write(base, block, (dst, Ty::I64));
            }
            _ => return Err(Unsupported::TypeMismatch { pc }),
        }
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
                pieces.push(to_display_str(ssa, insts, globals, v, ty, pc)?);
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
        }
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
        assert!(ir.contains("call ptr @lkrt_i64_to_str(i64"), "int display: {ir}");
        assert!(ir.contains("call ptr @lkrt_str_concat(ptr"), "then concat: {ir}");
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
