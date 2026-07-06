//! `lk-aot-mir` — the typed, SSA-form mid-level IR for LK's AOT backend.
//!
//! This crate sits between the bytecode `ModuleArtifact` and LLVM codegen. Its
//! whole reason to exist: make "can this program be lowered?" a *total, testable
//! predicate* (building an [`MirModule`] either succeeds or the lowering reports a
//! precise reason), and make codegen a *total* function (`MirModule -> LLVM`).
//!
//! Everything here is plain data with no LLVM or `lk-core` dependency — only the
//! shared ABI vocabulary from `lk-aot-abi`. Container/host operations are modelled
//! uniformly as [`Inst::Call`] to a named [`lk_aot_abi::AbiFn`], so codegen never
//! grows per-shape special cases.
//!
//! The type set ([`Ty`]) is deliberately closed: it *is* the definition of the
//! natively lowerable subset. A lowering that meets a value it cannot place into a
//! `Ty` rejects the program instead of silently widening the ABI.

use std::collections::HashSet;

/// SSA value handle (unique within a function).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ValueId(pub u32);

/// Basic-block handle (unique within a function).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockId(pub u32);

/// Function handle (unique within a module).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FuncId(pub u32);

/// Interned string/global constant handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct GlobalId(pub u32);

/// The closed set of natively-lowerable value types. Scalars plus (future)
/// container handles. Extending the AOT capability surface means adding a variant
/// here and a corresponding lowering + codegen arm.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ty {
    I64,
    F64,
    Bool,
    /// Owned/borrowed C-string pointer (`*const c_char` at the ABI).
    Str,
    Nil,
    /// A growable `List<i64>` handle (opaque `ptr` at the ABI). Phase 2 container
    /// handle-ification; more element types follow.
    ListI64,
    /// A growable `List<f64>` handle (opaque `ptr` at the ABI).
    ListF64,
    /// A growable `List<str>` handle (elements are `Str` pointers; opaque `ptr`).
    ListStr,
    /// A growable string-keyed `Map<str, i64>` handle (opaque `ptr` at the ABI).
    MapStrI64,
    /// A growable int-keyed `Map<i64, i64>` handle (opaque `ptr` at the ABI).
    MapI64I64,
    /// A growable string-keyed `Map<str, f64>` handle (opaque `ptr` at the ABI).
    MapStrF64,
    /// A growable int-keyed `Map<i64, f64>` handle (opaque `ptr` at the ABI).
    MapI64F64,
    /// A string-keyed `Map<str, bool>` handle. Values ride the `str_i64` map
    /// ABI as `0`/`1`; the type keeps bool display/compare semantics exact.
    MapStrBool,
    /// The result of a dynamic (not provably in-range) `List<i64>` index: a
    /// `Maybe<i64>` carried as an LLVM `{i64, i64}` (value, present). Its only
    /// supported consumer today is a function return (which prints the value or
    /// `nil`, matching the VM); using it in arithmetic rejects (falls back).
    MaybeI64,
    /// The `f64` analogue of [`Ty::MaybeI64`], carried as LLVM `{double, i64}`.
    MaybeF64,
    /// The `str` analogue of [`Ty::MaybeI64`], carried as LLVM `{ptr, i64}`.
    MaybeStr,
    /// The `bool` analogue of [`Ty::MaybeI64`]: carried as `{i64, i64}` (value
    /// `0`/`1`, present bit), narrowing to `Bool` on use.
    MaybeBool,
    /// A boxed dynamic value (`LkDyn { tag, payload }`, LLVM `{i64, i64}` by
    /// value): the escape hatch for genuinely mixed-type data (plan M4.2).
    /// Never appears on already-typed paths — lowering only boxes where the
    /// closed typed subset would otherwise reject.
    Dyn,
    /// A growable `List<LkDyn>` handle (opaque `ptr`): the mixed-element
    /// list backing `[1, "a", true]`-shaped literals.
    ListDyn,
    /// A growable string-keyed `Map<str, LkDyn>` handle (opaque `ptr`):
    /// mixed-value maps (`{"name": …, "age": 30, "active": true}`). Display
    /// stays out of the subset (hash order, docs/semantics.md).
    MapStrDyn,
}

/// Integer binary operators. `Div`/`Mod` are lowered to the divisor-guarded
/// `lkrt` helpers (never raw `sdiv`/`srem`), matching VM divide-by-zero semantics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
    Min,
    Max,
}

/// Float binary operators. `Div`/`Mod` are lowered to the guarded `lkrt` helpers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FloatBinOp {
    Add,
    Sub,
    Mul,
    Div,
    Mod,
}

/// Integer/float comparison predicates (result is a `Bool` value).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CmpOp {
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
}

/// A compile-time constant materialized into a value.
#[derive(Debug, Clone, PartialEq)]
pub enum Const {
    I64(i64),
    F64(f64),
    Bool(bool),
    /// References an interned module global (see [`MirModule::globals`]).
    Str(GlobalId),
    /// The address of a lowered user function (`ptr @lk_fn_N`) — passed to
    /// runtime helpers that invoke compiled callbacks (list HOF).
    FnAddr(FuncId),
    Nil,
}

/// A single SSA instruction: it defines at most one value (`dst`) from its inputs.
#[derive(Debug, Clone, PartialEq)]
pub enum Inst {
    /// `dst = <constant>`
    Const { dst: ValueId, value: Const },
    /// `dst = op(lhs, rhs)` on `I64`.
    IntBin {
        dst: ValueId,
        op: IntBinOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    /// `dst = op(lhs, rhs)` on `F64`.
    FloatBin {
        dst: ValueId,
        op: FloatBinOp,
        lhs: ValueId,
        rhs: ValueId,
    },
    /// `dst = (lhs <cmp> rhs)` producing a `Bool`. `float` selects int/float compare.
    Cmp {
        dst: ValueId,
        op: CmpOp,
        float: bool,
        lhs: ValueId,
        rhs: ValueId,
    },
    /// `dst = sitofp(src)` — widen an `I64` value to `F64`.
    IntToFloat { dst: ValueId, src: ValueId },
    /// `dst = zext(src)` — widen a `Bool` (`i1`) to `I64` (`0`/`1`).
    ZextBool { dst: ValueId, src: ValueId },
    /// `dst = !src` — boolean negation (`xor i1 src, true`).
    Not { dst: ValueId, src: ValueId },
    /// `dst = lhs & rhs` on `Bool` (`and i1`). Used by fused conjunction
    /// branches (`TestEqIntI2`).
    BoolAnd { dst: ValueId, lhs: ValueId, rhs: ValueId },
    /// `dst = (src.present != 0)` — extracts a `Maybe`'s present bit as a `Bool`
    /// (true ⇒ the value is present, i.e. not nil). `maybe_ty` is the operand's
    /// `Maybe` type, selecting the carrier struct for the `extractvalue`.
    MaybePresent { dst: ValueId, src: ValueId, maybe_ty: Ty },
    /// `dst? = callee(args)` — a call to a named ABI runtime function. Container and
    /// host operations funnel through here so codegen stays per-shape-free.
    Call {
        dst: Option<ValueId>,
        /// The `(module, name)` identity resolvable via [`lk_aot_abi::find`].
        callee: AbiRef,
        args: Vec<ValueId>,
    },
    /// `dst? = @lk_fn_{func}(args)` — a direct call to another function in this
    /// module (the native function ABI).
    CallFn {
        dst: Option<ValueId>,
        func: FuncId,
        args: Vec<ValueId>,
    },
    /// `call.vm f{func}(args)` — a one-way Tier 1 bridge call to a VM-executed
    /// function of this module (`docs/llvm/tier1-hybrid.md`): the callee's body
    /// did not lower, so codegen marshals the scalar arguments into tagged
    /// bridge values and calls `lk_hybrid_call_v`. Results never flow back
    /// (the lowering leaves the destination register unbound, so any use of
    /// the result rejects the module).
    CallVm { func: FuncId, args: Vec<ValueId> },
    /// `dst = lkrt_lklist_i64_get_pair(handle, index)` — a dynamic `List<i64>` read
    /// producing a [`Ty::MaybeI64`]. Kept a dedicated instruction (not a generic
    /// [`Inst::Call`]) because its `{i64, i64}` return is outside the scalar ABI
    /// vocabulary; codegen renders it specially.
    ListGetMaybe {
        dst: ValueId,
        handle: ValueId,
        index: ValueId,
    },
    /// `dst = lkrt_maybe_i64_unwrap(src.value, src.present)` — narrows a
    /// [`Ty::MaybeI64`] to an `I64` in a scalar context, aborting if the element was
    /// absent (matching the VM's halt on `nil` arithmetic). Emitted when a dynamic
    /// index result flows into arithmetic/comparison (never into a return, which
    /// keeps the `Maybe` to print `nil`).
    UnwrapMaybeI64 { dst: ValueId, src: ValueId },
    /// `dst = lkrt_lklist_f64_get_pair(handle, index)` — the `f64` analogue of
    /// [`Inst::ListGetMaybe`], producing a [`Ty::MaybeF64`].
    ListGetMaybeF64 {
        dst: ValueId,
        handle: ValueId,
        index: ValueId,
    },
    /// The `f64` analogue of [`Inst::UnwrapMaybeI64`] (narrows [`Ty::MaybeF64`] to
    /// `F64`, aborting if absent).
    UnwrapMaybeF64 { dst: ValueId, src: ValueId },
    /// `dst = lkrt_lklist_str_get_pair(handle, index)` — the `str` analogue of
    /// [`Inst::ListGetMaybe`], producing a [`Ty::MaybeStr`].
    ListGetMaybeStr {
        dst: ValueId,
        handle: ValueId,
        index: ValueId,
    },
    /// The `str` analogue of [`Inst::UnwrapMaybeI64`] (narrows [`Ty::MaybeStr`] to
    /// `Str`, aborting if absent).
    UnwrapMaybeStr { dst: ValueId, src: ValueId },
    /// `dst = lkrt_lkmap_str_i64_get_pair(handle, key)` — a `Map<str, i64>` lookup
    /// producing a [`Ty::MaybeI64`] (`present = 0` for a missing key). `key` is a
    /// `Str` value (an interned key global). Dedicated for the same reason as
    /// [`Inst::ListGetMaybe`]: its `{i64, i64}` return is outside the scalar ABI.
    MapGetMaybe {
        dst: ValueId,
        handle: ValueId,
        key: ValueId,
    },
    /// `dst = lkrt_lkmap_i64_i64_get_pair(handle, key)` — an int-keyed `Map<i64,i64>`
    /// lookup producing a [`Ty::MaybeI64`] (`present = 0` for a missing key). `key` is
    /// an `I64` value.
    MapGetMaybeI64Key {
        dst: ValueId,
        handle: ValueId,
        key: ValueId,
    },
    /// `dst = lkrt_lkmap_str_f64_get_pair(handle, key)` — a string-keyed `Map<str,f64>`
    /// lookup producing a [`Ty::MaybeF64`]. `key` is a `Str` value.
    MapGetMaybeStrF64 {
        dst: ValueId,
        handle: ValueId,
        key: ValueId,
    },
    /// `dst = lkrt_lkmap_i64_f64_get_pair(handle, key)` — an int-keyed `Map<i64,f64>`
    /// lookup producing a [`Ty::MaybeF64`]. `key` is an `I64` value.
    MapGetMaybeI64F64 {
        dst: ValueId,
        handle: ValueId,
        key: ValueId,
    },
    /// `printf("%s[\n]", value)` — prints a `Str` value to stdout, optionally with
    /// a trailing newline. Lowered from the `print`/`println` runtime builtins;
    /// the lowering display-converts and formats the arguments, so codegen only
    /// ever prints one finished string.
    PrintStr { value: ValueId, newline: bool },
    /// `dst = extractvalue src, 0` — reads a `Maybe` carrier's raw value
    /// **without** asserting presence (unlike [`Inst::UnwrapMaybeI64`]). Used
    /// on phi edges that merge a `Maybe` with a plain scalar: on the absent
    /// path the extracted value is never observed (the phi takes the other
    /// edge), so no abort semantics are required.
    MaybeValue { dst: ValueId, src: ValueId, maybe_ty: Ty },
    /// `dst = {src, 1}` — wraps a plain scalar into a present `Maybe` carrier
    /// (the dual of [`Inst::MaybeValue`] for mixed phi edges).
    MaybeWrap { dst: ValueId, src: ValueId, maybe_ty: Ty },
    /// `dst = select cond, then_v, else_v` over values of type `ty`.
    Select {
        dst: ValueId,
        cond: ValueId,
        then_v: ValueId,
        else_v: ValueId,
        ty: Ty,
    },
    /// `dst = load @lk_gvar_{gvar}` — reads a mutable module global (see
    /// [`MirModule::mutable_globals`]).
    GlobalGet { dst: ValueId, gvar: u32 },
    /// `store src, @lk_gvar_{gvar}` — writes a mutable module global.
    GlobalSet { gvar: u32, src: ValueId },
}

/// A resolved reference to an ABI function (its `(module, name)` identity).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AbiRef {
    pub module: &'static str,
    pub name: &'static str,
}

impl AbiRef {
    pub fn new(module: &'static str, name: &'static str) -> Self {
        Self { module, name }
    }

    /// Resolves this reference against the shared ABI schema.
    pub fn resolve(&self) -> Option<&'static lk_aot_abi::AbiFn> {
        lk_aot_abi::find(self.module, self.name)
    }
}

/// How a block ends. Every block has exactly one terminator.
#[derive(Debug, Clone, PartialEq)]
pub enum Term {
    /// Return an optional scalar value from the function.
    Ret(Option<ValueId>),
    /// Unconditional branch, passing block arguments to the target's params.
    Br { target: BlockId, args: Vec<ValueId> },
    /// Conditional branch on a `Bool` value.
    CondBr {
        cond: ValueId,
        then_blk: BlockId,
        then_args: Vec<ValueId>,
        else_blk: BlockId,
        else_args: Vec<ValueId>,
    },
    /// Diverge (matches how AOT lowers `panic` / a fatal guard) — `abort()`.
    Abort,
}

/// A basic block: SSA block parameters (the phi replacement) + instructions + one
/// terminator.
#[derive(Debug, Clone, PartialEq)]
pub struct Block {
    pub id: BlockId,
    pub params: Vec<(ValueId, Ty)>,
    pub insts: Vec<Inst>,
    pub term: Term,
}

/// A function in SSA form.
#[derive(Debug, Clone, PartialEq)]
pub struct MirFunction {
    pub id: FuncId,
    pub params: Vec<(ValueId, Ty)>,
    pub blocks: Vec<Block>,
    pub entry: BlockId,
    pub ret: Ty,
}

impl MirFunction {
    pub fn block(&self, id: BlockId) -> Option<&Block> {
        self.blocks.iter().find(|b| b.id == id)
    }
}

/// A whole lowered module.
#[derive(Debug, Clone, PartialEq)]
pub struct MirModule {
    /// ABI version this module is generated against (checked at runtime start).
    pub abi_version: i64,
    /// Interned string constants, addressed by [`GlobalId`].
    pub globals: Vec<String>,
    /// Mutable module-level variables (top-level `let`s shared with functions),
    /// addressed by index from [`Inst::GlobalGet`] / [`Inst::GlobalSet`]. The
    /// name is diagnostic only; codegen emits one typed LLVM global per entry.
    pub mutable_globals: Vec<(String, Ty)>,
    /// VM-executed functions (Tier 1 hybrid): reachable functions whose bodies
    /// did not lower but whose call sites bridge into the embedded VM. `params`
    /// are the scalar marshaling types for [`Inst::CallVm`] arguments.
    pub vm_functions: Vec<VmFunction>,
    pub functions: Vec<MirFunction>,
    pub entry: FuncId,
}

/// One VM-executed function of a Tier 1 hybrid module (see [`Inst::CallVm`]).
#[derive(Debug, Clone, PartialEq)]
pub struct VmFunction {
    pub id: FuncId,
    /// Scalar parameter types, in order — the bridge marshaling contract.
    pub params: Vec<Ty>,
}

impl MirModule {
    pub fn function(&self, id: FuncId) -> Option<&MirFunction> {
        self.functions.iter().find(|f| f.id == id)
    }

    pub fn vm_function(&self, id: FuncId) -> Option<&VmFunction> {
        self.vm_functions.iter().find(|f| f.id == id)
    }

    pub fn global(&self, id: GlobalId) -> Option<&str> {
        self.globals.get(id.0 as usize).map(String::as_str)
    }
}

/// Well-formedness violations found by [`validate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MirError {
    /// A value is used before it is defined (breaks SSA dominance-by-construction).
    UseBeforeDef { func: FuncId, value: ValueId },
    /// A value is defined more than once (breaks SSA single-assignment).
    RedefinedValue { func: FuncId, value: ValueId },
    /// A terminator/branch targets a block that does not exist.
    UnknownBlock { func: FuncId, block: BlockId },
    /// A `Call` names an ABI function absent from the schema.
    UnknownAbi { module: &'static str, name: &'static str },
    /// A `GlobalGet`/`GlobalSet` names a mutable global outside the module table.
    UnknownGlobal { func: FuncId, gvar: u32 },
    /// The module/function references a missing entry block/function.
    MissingEntry,
    /// A call or branch passes a different number of arguments than the
    /// callee's parameters / the target block's params expect.
    ArityMismatch { func: FuncId },
}

/// Validates structural well-formedness: single-assignment, define-before-use
/// within a linear block order, valid branch targets, and resolvable ABI calls.
///
/// This is intentionally conservative (it assumes blocks are listed in a
/// topological-ish order for the simple straightline/if shapes we lower first);
/// it is a cheap guard that catches lowering bugs long before LLVM would.
pub fn validate(module: &MirModule) -> Result<(), MirError> {
    if module.function(module.entry).is_none() {
        return Err(MirError::MissingEntry);
    }
    for func in &module.functions {
        if func.block(func.entry).is_none() {
            return Err(MirError::MissingEntry);
        }
        let mut defined: HashSet<ValueId> = HashSet::new();
        // Params and block params are defined on entry to their scope.
        for (v, _) in &func.params {
            if !defined.insert(*v) {
                return Err(MirError::RedefinedValue {
                    func: func.id,
                    value: *v,
                });
            }
        }
        let block_ids: HashSet<BlockId> = func.blocks.iter().map(|b| b.id).collect();
        for block in &func.blocks {
            for (v, _) in &block.params {
                if !defined.insert(*v) {
                    return Err(MirError::RedefinedValue {
                        func: func.id,
                        value: *v,
                    });
                }
            }
            for inst in &block.insts {
                for used in inst_uses(inst) {
                    if !defined.contains(&used) {
                        return Err(MirError::UseBeforeDef {
                            func: func.id,
                            value: used,
                        });
                    }
                }
                if let Inst::Call { callee, args, .. } = inst {
                    let Some(abi) = callee.resolve() else {
                        return Err(MirError::UnknownAbi {
                            module: callee.module,
                            name: callee.name,
                        });
                    };
                    if args.len() != abi.params.len() {
                        return Err(MirError::ArityMismatch { func: func.id });
                    }
                }
                if let Inst::GlobalGet { gvar, .. } | Inst::GlobalSet { gvar, .. } = inst
                    && *gvar as usize >= module.mutable_globals.len()
                {
                    return Err(MirError::UnknownGlobal {
                        func: func.id,
                        gvar: *gvar,
                    });
                }
                if let Inst::CallFn { func: callee, args, .. } = inst {
                    let Some(target) = module.function(*callee) else {
                        return Err(MirError::MissingEntry);
                    };
                    if args.len() != target.params.len() {
                        return Err(MirError::ArityMismatch { func: func.id });
                    }
                }
                if let Inst::CallVm { func: callee, args } = inst {
                    let Some(target) = module.vm_function(*callee) else {
                        return Err(MirError::MissingEntry);
                    };
                    if args.len() != target.params.len() {
                        return Err(MirError::ArityMismatch { func: func.id });
                    }
                }
                if let Some(def) = inst_def(inst)
                    && !defined.insert(def)
                {
                    return Err(MirError::RedefinedValue {
                        func: func.id,
                        value: def,
                    });
                }
            }
            for used in term_uses(&block.term) {
                if !defined.contains(&used) {
                    return Err(MirError::UseBeforeDef {
                        func: func.id,
                        value: used,
                    });
                }
            }
            for target in term_targets(&block.term) {
                if !block_ids.contains(&target) {
                    return Err(MirError::UnknownBlock {
                        func: func.id,
                        block: target,
                    });
                }
            }
            // Branch arguments must match the target block's params (the phi
            // inputs codegen will collect per predecessor).
            let target_arity = |target: BlockId| func.block(target).map(|b| b.params.len());
            match &block.term {
                Term::Br { target, args } => {
                    if target_arity(*target) != Some(args.len()) {
                        return Err(MirError::ArityMismatch { func: func.id });
                    }
                }
                Term::CondBr {
                    then_blk,
                    then_args,
                    else_blk,
                    else_args,
                    ..
                } => {
                    if target_arity(*then_blk) != Some(then_args.len())
                        || target_arity(*else_blk) != Some(else_args.len())
                    {
                        return Err(MirError::ArityMismatch { func: func.id });
                    }
                }
                Term::Ret(_) | Term::Abort => {}
            }
        }
    }
    Ok(())
}

/// Renders a module as stable, review-friendly text — the snapshot surface for
/// lowering tests (RFC §6). The format is deliberately line-oriented and free of
/// LLVM syntax so snapshots survive codegen changes.
pub fn render(module: &MirModule) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(1024);
    let _ = writeln!(out, "mir module (abi v{})", module.abi_version);
    for (i, g) in module.globals.iter().enumerate() {
        let _ = writeln!(out, "global g{i} = {g:?}");
    }
    for vm_fn in &module.vm_functions {
        let params = vm_fn
            .params
            .iter()
            .map(|ty| ty_name(*ty))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "vm fn f{}({params})", vm_fn.id.0);
    }
    for func in &module.functions {
        let entry = if func.id == module.entry { " entry" } else { "" };
        let params = func
            .params
            .iter()
            .map(|(v, ty)| format!("v{}: {}", v.0, ty_name(*ty)))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(out, "fn f{}({params}) -> {}{entry} {{", func.id.0, ty_name(func.ret));
        for block in &func.blocks {
            let bparams = block
                .params
                .iter()
                .map(|(v, ty)| format!("v{}: {}", v.0, ty_name(*ty)))
                .collect::<Vec<_>>()
                .join(", ");
            let _ = writeln!(out, "bb{}({bparams}):", block.id.0);
            for inst in &block.insts {
                let _ = writeln!(out, "  {}", render_inst(inst));
            }
            let _ = writeln!(out, "  {}", render_term(&block.term));
        }
        let _ = writeln!(out, "}}");
    }
    out
}

fn ty_name(ty: Ty) -> &'static str {
    match ty {
        Ty::I64 => "i64",
        Ty::F64 => "f64",
        Ty::Bool => "bool",
        Ty::Str => "str",
        Ty::Nil => "nil",
        Ty::ListI64 => "list<i64>",
        Ty::ListF64 => "list<f64>",
        Ty::ListStr => "list<str>",
        Ty::MapStrI64 => "map<str,i64>",
        Ty::MapI64I64 => "map<i64,i64>",
        Ty::MapStrF64 => "map<str,f64>",
        Ty::MapI64F64 => "map<i64,f64>",
        Ty::MapStrBool => "map<str,bool>",
        Ty::MaybeI64 => "maybe<i64>",
        Ty::MaybeF64 => "maybe<f64>",
        Ty::MaybeStr => "maybe<str>",
        Ty::MaybeBool => "maybe<bool>",
        Ty::Dyn => "dyn",
        Ty::ListDyn => "list<dyn>",
        Ty::MapStrDyn => "map<str,dyn>",
    }
}

fn render_inst(inst: &Inst) -> String {
    fn v(id: ValueId) -> String {
        format!("v{}", id.0)
    }
    fn args(list: &[ValueId]) -> String {
        list.iter().map(|a| format!("v{}", a.0)).collect::<Vec<_>>().join(", ")
    }
    match inst {
        Inst::Const { dst, value } => {
            let c = match value {
                Const::I64(n) => format!("const.i64 {n}"),
                Const::F64(x) => format!("const.f64 {x:?}"),
                Const::Bool(b) => format!("const.bool {b}"),
                Const::Str(g) => format!("const.str g{}", g.0),
                Const::FnAddr(f) => format!("const.fnaddr fn{}", f.0),
                Const::Nil => "const.nil".to_string(),
            };
            format!("{} = {c}", v(*dst))
        }
        Inst::IntBin { dst, op, lhs, rhs } => {
            format!(
                "{} = int.{} {}, {}",
                v(*dst),
                format!("{op:?}").to_lowercase(),
                v(*lhs),
                v(*rhs)
            )
        }
        Inst::FloatBin { dst, op, lhs, rhs } => {
            format!(
                "{} = float.{} {}, {}",
                v(*dst),
                format!("{op:?}").to_lowercase(),
                v(*lhs),
                v(*rhs)
            )
        }
        Inst::Cmp {
            dst,
            op,
            float,
            lhs,
            rhs,
        } => {
            let kind = if *float { "fcmp" } else { "icmp" };
            format!(
                "{} = {kind}.{} {}, {}",
                v(*dst),
                format!("{op:?}").to_lowercase(),
                v(*lhs),
                v(*rhs)
            )
        }
        Inst::IntToFloat { dst, src } => format!("{} = sitofp {}", v(*dst), v(*src)),
        Inst::ZextBool { dst, src } => format!("{} = zext.bool {}", v(*dst), v(*src)),
        Inst::Not { dst, src } => format!("{} = not {}", v(*dst), v(*src)),
        Inst::BoolAnd { dst, lhs, rhs } => format!("{} = bool.and {}, {}", v(*dst), v(*lhs), v(*rhs)),
        Inst::MaybePresent { dst, src, maybe_ty } => {
            format!("{} = maybe.present<{}> {}", v(*dst), ty_name(*maybe_ty), v(*src))
        }
        Inst::Call { dst, callee, args: a } => {
            let call = format!("call {}.{}({})", callee.module, callee.name, args(a));
            match dst {
                Some(d) => format!("{} = {call}", v(*d)),
                None => call,
            }
        }
        Inst::CallFn { dst, func, args: a } => {
            let call = format!("call f{}({})", func.0, args(a));
            match dst {
                Some(d) => format!("{} = {call}", v(*d)),
                None => call,
            }
        }
        Inst::CallVm { func, args: a } => format!("call.vm f{}({})", func.0, args(a)),
        Inst::ListGetMaybe { dst, handle, index } => {
            format!("{} = list.i64.get_maybe {}, {}", v(*dst), v(*handle), v(*index))
        }
        Inst::UnwrapMaybeI64 { dst, src } => format!("{} = maybe.i64.unwrap {}", v(*dst), v(*src)),
        Inst::ListGetMaybeF64 { dst, handle, index } => {
            format!("{} = list.f64.get_maybe {}, {}", v(*dst), v(*handle), v(*index))
        }
        Inst::UnwrapMaybeF64 { dst, src } => format!("{} = maybe.f64.unwrap {}", v(*dst), v(*src)),
        Inst::ListGetMaybeStr { dst, handle, index } => {
            format!("{} = list.str.get_maybe {}, {}", v(*dst), v(*handle), v(*index))
        }
        Inst::UnwrapMaybeStr { dst, src } => format!("{} = maybe.str.unwrap {}", v(*dst), v(*src)),
        Inst::MapGetMaybe { dst, handle, key } => {
            format!("{} = map.str_i64.get_maybe {}, {}", v(*dst), v(*handle), v(*key))
        }
        Inst::MapGetMaybeI64Key { dst, handle, key } => {
            format!("{} = map.i64_i64.get_maybe {}, {}", v(*dst), v(*handle), v(*key))
        }
        Inst::MapGetMaybeStrF64 { dst, handle, key } => {
            format!("{} = map.str_f64.get_maybe {}, {}", v(*dst), v(*handle), v(*key))
        }
        Inst::MapGetMaybeI64F64 { dst, handle, key } => {
            format!("{} = map.i64_f64.get_maybe {}, {}", v(*dst), v(*handle), v(*key))
        }
        Inst::PrintStr { value, newline } => {
            format!("print.str{} {}", if *newline { "ln" } else { "" }, v(*value))
        }
        Inst::Select {
            dst,
            cond,
            then_v,
            else_v,
            ..
        } => format!("{} = select {}, {}, {}", v(*dst), v(*cond), v(*then_v), v(*else_v)),
        Inst::MaybeValue { dst, src, maybe_ty } => {
            format!("{} = maybe.value<{}> {}", v(*dst), ty_name(*maybe_ty), v(*src))
        }
        Inst::MaybeWrap { dst, src, maybe_ty } => {
            format!("{} = maybe.wrap<{}> {}", v(*dst), ty_name(*maybe_ty), v(*src))
        }
        Inst::GlobalGet { dst, gvar } => format!("{} = gvar.get gvar{gvar}", v(*dst)),
        Inst::GlobalSet { gvar, src } => format!("gvar.set gvar{gvar}, {}", v(*src)),
    }
}

fn render_term(term: &Term) -> String {
    fn args(list: &[ValueId]) -> String {
        list.iter().map(|a| format!("v{}", a.0)).collect::<Vec<_>>().join(", ")
    }
    match term {
        Term::Ret(None) => "ret".to_string(),
        Term::Ret(Some(v)) => format!("ret v{}", v.0),
        Term::Br { target, args: a } => format!("br bb{}({})", target.0, args(a)),
        Term::CondBr {
            cond,
            then_blk,
            then_args,
            else_blk,
            else_args,
        } => format!(
            "condbr v{}, bb{}({}), bb{}({})",
            cond.0,
            then_blk.0,
            args(then_args),
            else_blk.0,
            args(else_args)
        ),
        Term::Abort => "abort".to_string(),
    }
}

fn inst_def(inst: &Inst) -> Option<ValueId> {
    match inst {
        Inst::Const { dst, .. }
        | Inst::IntBin { dst, .. }
        | Inst::FloatBin { dst, .. }
        | Inst::Cmp { dst, .. }
        | Inst::IntToFloat { dst, .. }
        | Inst::ZextBool { dst, .. }
        | Inst::Not { dst, .. }
        | Inst::BoolAnd { dst, .. }
        | Inst::MaybePresent { dst, .. }
        | Inst::ListGetMaybe { dst, .. }
        | Inst::UnwrapMaybeI64 { dst, .. }
        | Inst::ListGetMaybeF64 { dst, .. }
        | Inst::UnwrapMaybeF64 { dst, .. }
        | Inst::ListGetMaybeStr { dst, .. }
        | Inst::UnwrapMaybeStr { dst, .. }
        | Inst::MapGetMaybe { dst, .. }
        | Inst::MapGetMaybeI64Key { dst, .. }
        | Inst::MapGetMaybeStrF64 { dst, .. }
        | Inst::MapGetMaybeI64F64 { dst, .. }
        | Inst::MaybeValue { dst, .. }
        | Inst::MaybeWrap { dst, .. }
        | Inst::Select { dst, .. }
        | Inst::GlobalGet { dst, .. } => Some(*dst),
        Inst::Call { dst, .. } | Inst::CallFn { dst, .. } => *dst,
        Inst::PrintStr { .. } | Inst::GlobalSet { .. } | Inst::CallVm { .. } => None,
    }
}

fn inst_uses(inst: &Inst) -> Vec<ValueId> {
    match inst {
        Inst::Const { .. } => vec![],
        Inst::IntBin { lhs, rhs, .. }
        | Inst::FloatBin { lhs, rhs, .. }
        | Inst::Cmp { lhs, rhs, .. }
        | Inst::BoolAnd { lhs, rhs, .. } => {
            vec![*lhs, *rhs]
        }
        Inst::IntToFloat { src, .. }
        | Inst::ZextBool { src, .. }
        | Inst::Not { src, .. }
        | Inst::MaybePresent { src, .. }
        | Inst::UnwrapMaybeI64 { src, .. }
        | Inst::UnwrapMaybeF64 { src, .. }
        | Inst::UnwrapMaybeStr { src, .. }
        | Inst::MaybeValue { src, .. }
        | Inst::MaybeWrap { src, .. } => {
            vec![*src]
        }
        Inst::ListGetMaybe { handle, index, .. }
        | Inst::ListGetMaybeF64 { handle, index, .. }
        | Inst::ListGetMaybeStr { handle, index, .. } => {
            vec![*handle, *index]
        }
        Inst::MapGetMaybe { handle, key, .. }
        | Inst::MapGetMaybeI64Key { handle, key, .. }
        | Inst::MapGetMaybeStrF64 { handle, key, .. }
        | Inst::MapGetMaybeI64F64 { handle, key, .. } => {
            vec![*handle, *key]
        }
        Inst::Call { args, .. } | Inst::CallFn { args, .. } | Inst::CallVm { args, .. } => args.clone(),
        Inst::PrintStr { value, .. } => vec![*value],
        Inst::Select {
            cond, then_v, else_v, ..
        } => vec![*cond, *then_v, *else_v],
        Inst::GlobalGet { .. } => vec![],
        Inst::GlobalSet { src, .. } => vec![*src],
    }
}

fn term_uses(term: &Term) -> Vec<ValueId> {
    match term {
        Term::Ret(v) => v.iter().copied().collect(),
        Term::Br { args, .. } => args.clone(),
        Term::CondBr {
            cond,
            then_args,
            else_args,
            ..
        } => {
            let mut v = vec![*cond];
            v.extend(then_args.iter().copied());
            v.extend(else_args.iter().copied());
            v
        }
        Term::Abort => vec![],
    }
}

fn term_targets(term: &Term) -> Vec<BlockId> {
    match term {
        Term::Ret(_) | Term::Abort => vec![],
        Term::Br { target, .. } => vec![*target],
        Term::CondBr { then_blk, else_blk, .. } => vec![*then_blk, *else_blk],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `fn main() -> i64 { return 20 / 4 }` (division goes through the ABI helper).
    fn div_module() -> MirModule {
        let (a, b, out) = (ValueId(0), ValueId(1), ValueId(2));
        MirModule {
            abi_version: lk_aot_abi::ABI_VERSION,
            globals: vec![],
            mutable_globals: Vec::new(),
            vm_functions: Vec::new(),
            entry: FuncId(0),
            functions: vec![MirFunction {
                id: FuncId(0),
                params: vec![],
                entry: BlockId(0),
                ret: Ty::I64,
                blocks: vec![Block {
                    id: BlockId(0),
                    params: vec![],
                    insts: vec![
                        Inst::Const {
                            dst: a,
                            value: Const::I64(20),
                        },
                        Inst::Const {
                            dst: b,
                            value: Const::I64(4),
                        },
                        Inst::IntBin {
                            dst: out,
                            op: IntBinOp::Div,
                            lhs: a,
                            rhs: b,
                        },
                    ],
                    term: Term::Ret(Some(out)),
                }],
            }],
        }
    }

    #[test]
    fn valid_module_passes() {
        assert_eq!(validate(&div_module()), Ok(()));
    }

    #[test]
    fn use_before_def_is_rejected() {
        let mut m = div_module();
        // Swap so the division uses `out` before it is defined.
        m.functions[0].blocks[0].insts[2] = Inst::IntBin {
            dst: ValueId(2),
            op: IntBinOp::Div,
            lhs: ValueId(9),
            rhs: ValueId(1),
        };
        assert_eq!(
            validate(&m),
            Err(MirError::UseBeforeDef {
                func: FuncId(0),
                value: ValueId(9)
            })
        );
    }

    #[test]
    fn unknown_branch_target_is_rejected() {
        let mut m = div_module();
        m.functions[0].blocks[0].term = Term::Br {
            target: BlockId(7),
            args: vec![],
        };
        assert_eq!(
            validate(&m),
            Err(MirError::UnknownBlock {
                func: FuncId(0),
                block: BlockId(7)
            })
        );
    }

    #[test]
    fn div_lowers_to_a_known_abi_helper() {
        // The MIR div op is expected to resolve to the guarded lkrt helper.
        assert!(AbiRef::new("arith", "i64_div").resolve().is_some());
    }
}
