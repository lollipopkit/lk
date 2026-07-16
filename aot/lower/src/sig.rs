use super::*;

/// Inferred function signatures, refined to a fixpoint before the final lowering.
///
/// User functions use a monomorphic `(params...) -> ret` native ABI. Neither the
/// parameter types nor the return type are in the bytecode, so both are inferred:
///  - `ret_types[f]` — from `f`'s return value type (returns can chain: `f` returns
///    `g()`), so it iterates.
///  - `param_obs[f][i]` — the argument type observed at `f`'s `CallDirect` sites. If
///    every site agrees, that is the parameter type; disagreeing sites join the
///    parameter to `Dyn` (each site boxes, the body consumes through the Dyn
///    arms — plan M4.2 cross-function Dyn flow). `conflict` still rejects
///    function-vs-value polymorphism (`lambda_params`/`specialized`).
pub(crate) struct SigInfer {
    pub(crate) param_obs: Vec<Vec<Option<Ty>>>,
    pub(crate) ret_types: Vec<Ty>,
    /// Whether `ret_types[f]` reflects an actual lowering of `f`'s body (vs
    /// the pristine `I64` default): HOF re-route decisions must not treat the
    /// default as a real mismatch.
    pub(crate) ret_known: Vec<bool>,
    pub(crate) conflict: bool,
    /// Empty-`[]` literals whose guessed element type a consumer
    /// contradicted (`(function, pc)`): the next fixpoint pass materializes
    /// them as Dyn lists.
    pub(crate) dyn_empty_lists: std::collections::HashSet<(u32, usize)>,
    /// Loop-header phis discovered to merge heterogeneous boxable types
    /// (`(function, block, slot)`): the next fixpoint pass pre-types them
    /// `Dyn` so the loop body consumes them through the Dyn arms.
    pub(crate) dyn_loop_phis: std::collections::HashSet<(u32, usize, usize)>,
    /// Functions whose returns disagreed on a boxable type (or returned a
    /// nullable carrier): the next fixpoint pass boxes every return point,
    /// making the function return `Dyn` instead of rejecting the module.
    pub(crate) dyn_rets: std::collections::HashSet<u32>,
    /// Per module-global slot: the scalar type every `SetGlobal` writes (a
    /// mixed-type global marks `conflict`, rejecting the module rather than
    /// miscompiling one of the writes).
    pub(crate) global_tys: Vec<Option<Ty>>,
    /// Slots written by the entry function *before* any control flow or user
    /// call. Only these may be read via `GetGlobal`: the VM initializes
    /// globals to nil while native storage zero-initializes, so a read that
    /// could observe the pre-first-write value must reject.
    pub(crate) initialized_globals: Vec<bool>,
    /// Slots holding a top-level capture-free closure (`let f = |x| …`):
    /// assigned exactly once, in the entry prefix, from a zero-capture
    /// `MakeClosure`. Reading such a slot yields [`GlobalRef::Lambda`].
    pub(crate) lambda_globals: Vec<Option<u32>>,
    /// `lambda_params[f][i]` — this function's i-th parameter is an *erased*
    /// lambda with a statically known identity: the callee seeds the register
    /// with a `GlobalRef::Lambda`/`Closure` instead of binding a value, so
    /// indirect calls through it devirtualize. A capturing identity adds
    /// hidden environment parameters (after the visible ones, before the
    /// callee's own captures). Set on clone materialization.
    pub(crate) lambda_params: Vec<Vec<Option<LambdaIdentity>>>,
    /// Clone specialization table: `(original fn, lambda identity per param)`
    /// → the specialized clone's id. Call sites passing lambdas retarget to
    /// the clone whose identity vector matches, so *different* lambdas at the
    /// same parameter get independent clones instead of a conflict.
    pub(crate) specializations: std::collections::HashMap<(u32, Vec<Option<LambdaIdentity>>), u32>,
    /// Clones queued during a pass (original fn ids, in id-assignment order),
    /// materialized into the working function list between passes.
    pub(crate) pending_clones: Vec<u32>,
    /// Original functions that have at least one specialized (lambda-passing)
    /// call site. If such a function also has a plain call site
    /// (`plain_called`), the program is polymorphic over functions vs values —
    /// reject. Otherwise the original body is skipped (all callers use clones).
    pub(crate) specialized: Vec<bool>,
    /// Original functions with at least one all-plain call site.
    pub(crate) plain_called: Vec<bool>,
    /// `ret_closures[f]` — this function's single return is a closure whose
    /// captures all map to its parameters: `(lambda fidx, capture sources)`.
    /// Call sites consume the summary (the result register is seeded with the
    /// closure ref, no call emitted); the pure body is never emitted.
    pub(crate) ret_closures: Vec<Option<(u32, Vec<RetCaptureSrc>)>>,
    /// Functions whose returns disagreed with a recorded summary — a poisoned
    /// function never records again and rejects on lowering instead.
    pub(crate) ret_closure_poisoned: Vec<bool>,
    /// Diagnostic names for the mutable-global table (slot-indexed).
    pub(crate) global_names: Vec<String>,
    /// Final compact `slot → gvar` numbering, built once signatures converge
    /// (empty during the fixpoint passes, whose emitted MIR is discarded).
    pub(crate) gvar_of: std::collections::HashMap<u16, u32>,
    /// Tier 1 hybrid: functions whose bodies did not lower but whose call
    /// sites bridge into the embedded VM (`fidx → scalar marshaling types`).
    /// Empty during the fixpoint; filled between the failing final pass and
    /// its hybrid retry (`docs/llvm/tier1-hybrid.md`).
    pub(crate) vm_functions: std::collections::HashMap<u32, Vec<Ty>>,
    /// Import-derived name bindings (aliases, module items, bundled files).
    pub(crate) imports: ImportEnv,
    /// Trait/impl registrations lifted from the entry (plan J1).
    pub(crate) traits: TraitEnv,
    /// Global slots first written *outside* the entry prefix but read via
    /// `GetGlobal` (`fn inc() { counter += 1; }` over a mid-entry `let`):
    /// forced to the `Dyn` carrier — its zero-initialization `{0, 0}` *is*
    /// the nil tag, so a read before the first write observes the VM's nil
    /// instead of a bogus typed zero. Retriable discovery (fixpoint rerun).
    pub(crate) force_dyn_globals: std::collections::HashSet<u16>,
    /// Functions spawned as goroutines (isolate semantics): their cell
    /// captures snapshot by value and cell *writes* land in a
    /// thread-private virtual slot instead of rejecting.
    pub(crate) spawned_isolate: std::collections::HashSet<u32>,
}

impl SigInfer {
    pub(crate) fn param_ty(&self, func: usize, i: usize) -> Ty {
        self.param_obs[func].get(i).copied().flatten().unwrap_or(Ty::I64)
    }

    /// Records one call-site observation of `callee`'s parameter `slot_idx`
    /// and returns the parameter's (possibly widened) type. Disagreeing
    /// observations join to `Dyn` — the parameter becomes dynamically typed
    /// and every call site boxes — instead of rejecting the module. Nullable
    /// shapes (`Nil`, the `Maybe` carriers) have no typed parameter form and
    /// observe as `Dyn` directly. The join is monotonic on a two-level
    /// lattice, so the fixpoint still terminates; function-vs-value
    /// polymorphism keeps its own reject (`lambda_params`).
    pub(crate) fn observe_param(&mut self, callee: usize, slot_idx: usize, arg_ty: Ty) -> Ty {
        let obs = match arg_ty {
            Ty::Nil | Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => Ty::Dyn,
            other => other,
        };
        match self.param_obs.get_mut(callee).and_then(|p| p.get_mut(slot_idx)) {
            Some(slot) => {
                let joined = match *slot {
                    None => obs,
                    Some(prev) if prev == obs => prev,
                    Some(_) => Ty::Dyn,
                };
                *slot = Some(joined);
                joined
            }
            None => obs,
        }
    }

    pub(crate) fn gvar(&self, slot: u16) -> u32 {
        self.gvar_of.get(&slot).copied().unwrap_or(u32::from(slot))
    }
}

/// A `Ret` of a register holding a closure ref whose captures all resolve
/// (in the returning block) to the function's own parameter values.
pub(crate) fn ret_closure_candidate(
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
pub(crate) fn ret_closure_body_is_pure(instrs: &[Instr]) -> bool {
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
pub(crate) fn record_ret_closure(sig: &mut SigInfer, fi: usize, candidate: (u32, Vec<RetCaptureSrc>)) {
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
