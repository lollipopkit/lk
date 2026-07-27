//! `lk-aot-lower` — the bytecode → typed MIR bridge.
//!
//! This is the *total capability predicate* of the redesign: [`lower`] turns a
//! bytecode [`ModuleArtifact`] into an [`MirModule`] or returns a precise
//! [`Unsupported`] reason. Nothing downstream (`lk-aot-codegen`) can fail, because
//! anything not expressible in the closed MIR type set is rejected right here.
//!
//! Scope today: every `examples/` program lowers fully native (the
//! `AOT_COVERAGE_REQUIRE_FULL` gate) — scalars and typed arithmetic, full
//! acyclic + reducible-loop control flow, list/map/set/object handles, boxed
//! `Dyn` values, strings, closures and captures, mutable module globals, trait
//! dispatch, and `try`. What stays out is enumerated by [`Unsupported`], and a
//! reachable function that fails may still be bridged to the embedded VM
//! (Tier 1 hybrid, `docs/aot/tier1-hybrid.md`) instead of failing the module.
//!
//! Control flow uses **on-demand SSA construction** (Braun et al.):
//! `Test`/`BrTrue`/`BrFalse`/`Jmp` and fused `TestXxxInt(I)`+`Jmp` become MIR
//! blocks with `CondBr`/`Br`; registers live across a merge or a loop back-edge
//! become block params, with incomplete phis in unsealed (loop-header) blocks
//! filled once the back-edge predecessor is lowered. Trivial-phi elimination is
//! intentionally omitted: the constructed SSA is correct but not minimal.
//!
//! Module map:
//! - [`inst`] — per-instruction lowering, routed by semantic family
//!   (scalar / string / call / global / container / control).
//! - `function` / `cfg` / `ssa` — block splitting, terminators, SSA construction.
//! - `convert` — register reads in a scalar context, numeric coercion, display
//!   conversion (the `Maybe` unwrap rules live here).
//! - `prescan` / `sig` — whole-module facts gathered before lowering: parameter
//!   and return observations, global slot types, bridge eligibility.
//! - `lower_call` / `lower_method` / `lower_builtin` / `lower_module` — callee
//!   resolution for user functions, method dispatch, builtins, stdlib modules.
//!
//! See `docs/aot/aot-redesign.md` §7/§9.5.

use std::collections::BTreeMap;

use lk_aot_mir::{
    AbiRef, Block, BlockId, CmpOp, Const, FloatBinOp, FuncId, GlobalId, Inst, IntBinOp, MirFunction, MirModule, Term,
    Ty, ValueId, VmFunction,
};
use lk_core::vm::{
    ConstHeapValueData, ConstRuntimeValueData, FunctionData, Instr, ModuleArtifact, Opcode, RuntimeMapKeyData,
};

mod cfg;
mod convert;
mod dyn_box;
mod function;
mod imports;
mod inst;
mod lower_builtin;
mod lower_call;
mod lower_method;
mod lower_module;
mod ops;
mod prescan;
mod sig;
mod ssa;
mod tables;
#[cfg(test)]
mod tests;
mod trait_env;
mod try_region;
mod unsupported;
mod vocab;

pub use self::imports::BundledImport;
pub(crate) use self::imports::ImportEnv;
pub use self::unsupported::Unsupported;
pub(crate) use self::{
    cfg::*, convert::*, dyn_box::*, function::*, inst::*, lower_builtin::*, lower_call::*, lower_method::*,
    lower_module::*, ops::*, prescan::*, sig::*, ssa::*, tables::*, trait_env::*, vocab::*,
};

type Reg = (ValueId, Ty);

pub fn lower(artifact: &ModuleArtifact) -> Result<MirModule, Unsupported> {
    // Default-on since the nightly correctness rounds went green on v1+v2
    // (tier1-hybrid.md): `LK_AOT_HYBRID=0` opts out (pure whole-module
    // native-or-fallback, e.g. the coverage script's metric).
    let hybrid = std::env::var_os("LK_AOT_HYBRID").is_none_or(|value| value != "0");
    lower_with_hybrid(artifact, hybrid)
}

/// [`lower`] with the Tier 1 hybrid mode passed explicitly (tests use this to
/// avoid process-global env mutation): when `hybrid` is set, a reachable
/// non-entry function whose body does not lower can be marked *VM-executed*
/// instead of failing the module, provided it is bridge-eligible (scalar
/// parameters, no captures or lambda machinery, transitively user-global-free
/// — see `docs/aot/tier1-hybrid.md`).
pub fn lower_with_hybrid(artifact: &ModuleArtifact, hybrid: bool) -> Result<MirModule, Unsupported> {
    lower_bundled(artifact, &[], hybrid)
}

/// [`lower_with_hybrid`] over an artifact whose function table already
/// contains compile-time-bundled file imports (multi-file `lk compile`).
pub fn lower_bundled(
    artifact: &ModuleArtifact,
    bundles: &[BundledImport],
    hybrid: bool,
) -> Result<MirModule, Unsupported> {
    let module = &artifact.module;
    if module.functions.is_empty() {
        return Err(Unsupported::NoEntry);
    }
    let n = module.functions.len();

    // Reachability from the entry via `CallDirect`. Functions that are defined but
    // never directly called (e.g. a small helper the front end inlined at every use)
    // are dead for AOT; lowering them would pointlessly fail the whole module if they
    // use a shape we don't support, so we skip them entirely.
    // Bundled file-module functions are reached through import bindings
    // (`GetGlobal` → Lambda), invisible to the bytecode scan: root them too
    // (the BFS then covers their own callees).
    // Bundled file-module functions are reached through import bindings
    // (`GetGlobal` → Lambda), invisible to the bytecode scan: root them too
    // (the BFS then covers their own callees).
    let mut bundle_roots: Vec<usize> = bundles
        .iter()
        .flat_map(|b| b.fns.values().map(|&fidx| fidx as usize))
        .collect();
    // Trait impl methods are reached through the lifted registration table
    // (their `LoadFunction` sites are skipped), invisible to the CallDirect/
    // MakeClosure scan: root them like bundled imports.
    let traits = trait_env_prescan(module);
    bundle_roots.extend(traits.impls.values().map(|&fidx| fidx as usize));
    let mut reachable = reachable_functions(module, &bundle_roots);

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
        ret_known: vec![false; n],
        conflict: false,
        dyn_loop_phis: std::collections::HashSet::new(),
        dyn_rets: std::collections::HashSet::new(),
        imports: ImportEnv::build(&artifact.imports, bundles)?,
        traits,
        force_dyn_globals: std::collections::HashSet::new(),
        spawned_isolate: std::collections::HashSet::new(),
        dyn_empty_lists: std::collections::HashSet::new(),
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
    // A closure so the hybrid rerun below can re-converge: marking a function
    // VM-executed changes the type lattice (its callers now see the bridge's
    // `Dyn` result, which widens their own params and returns), and emitting
    // against the pre-marking signatures is how a caller ended up calling
    // `str.from_i64` with a register pair.
    let refine_signatures = |sig: &mut SigInfer, funcs: &mut Vec<FunctionData>, reachable: &mut Vec<bool>| {
        let mut passes = 0usize;
        loop {
            let snapshot = (
                sig.param_obs.clone(),
                sig.ret_types.clone(),
                sig.specializations.len(),
                sig.ret_closures.clone(),
                sig.dyn_loop_phis.len(),
                sig.dyn_empty_lists.len(),
                sig.dyn_rets.len(),
                sig.global_tys.clone(),
                sig.spawned_isolate.len(),
                sig.force_dyn_globals.len(),
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
                let lowered = lower_function(
                    &funcs[fi],
                    funcs.as_slice(),
                    fi as u32,
                    module.entry,
                    is_entry,
                    &mut scratch,
                    &module.globals,
                    sig,
                );
                match lowered {
                    Ok(mf) if !is_entry => {
                        sig.ret_types[fi] = mf.ret;
                        sig.ret_known[fi] = true;
                    }
                    // A retriable loop-phi discovery: record it (the snapshot
                    // includes the set's size, so the fixpoint runs again with
                    // the phi pre-typed Dyn).
                    Err(Unsupported::DynLoopPhi { block, slot }) => {
                        sig.dyn_loop_phis.insert((fi as u32, block, slot));
                    }
                    Err(Unsupported::EmptyListGuessWrong { pcs }) => {
                        for pc in pcs {
                            sig.dyn_empty_lists.insert((fi as u32, pc));
                        }
                    }
                    _ => {}
                }
            }
            // Materialize clones queued during this pass so the next pass lowers
            // them (their `lambda_params` are already in place).
            for orig in std::mem::take(&mut sig.pending_clones) {
                funcs.push(funcs[orig as usize].clone());
                reachable.push(true);
            }
            passes += 1;
            // Field-by-field comparison against the pre-pass snapshot: the same
            // convergence condition without cloning the whole state a second
            // time. (A generation-counter scheme was evaluated and rejected:
            // ~30 mutation sites to instrument, and one missed bump = false
            // convergence = miscompile; the snapshot clone stays as the
            // correctness anchor.)
            let converged = snapshot.0 == sig.param_obs
                && snapshot.1 == sig.ret_types
                && snapshot.2 == sig.specializations.len()
                && snapshot.3 == sig.ret_closures
                && snapshot.4 == sig.dyn_loop_phis.len()
                && snapshot.5 == sig.dyn_empty_lists.len()
                && snapshot.6 == sig.dyn_rets.len()
                && snapshot.7 == sig.global_tys
                && snapshot.8 == sig.spawned_isolate.len()
                && snapshot.9 == sig.force_dyn_globals.len();
            // Each retriable discovery (Dyn loop phi, empty-list re-guess,
            // boxed-returns function) legitimately consumes one extra pass, so
            // the safety valve budgets for them on top of the type lattice.
            let discovery_budget =
                sig.dyn_loop_phis.len() + sig.dyn_empty_lists.len() + sig.dyn_rets.len() + sig.force_dyn_globals.len();
            if converged || passes > 2 * funcs.len() + 2 + discovery_budget {
                break;
            }
        }
    };
    refine_signatures(&mut sig, &mut funcs, &mut reachable);
    // A conflict that survives the *converged* pass is real (per-pass resets
    // clear transient marks left before a closure-return summary landed):
    // param-type disagreement or function-vs-value polymorphism.
    if sig.conflict {
        return Err(Unsupported::ReturnTypeConflict);
    }

    // Compact numbering for the mutable globals the fixpoint discovered; the
    // final pass emits `GlobalGet`/`GlobalSet` against these ids.
    //
    // Recomputed from scratch whenever signatures are re-converged, because
    // lowering *discovers* globals: `inst/global.rs` fills `global_tys` as it
    // sees accesses (`None → Some(ty)`, and a second, different observation
    // widens to `Dyn`). Reusing the pre-rerun numbering would leave a
    // newly-typed slot with no `gvar_of` entry, and `SigInfer::gvar` falls back
    // to the raw slot number — which can *collide* with a compacted id and
    // alias two distinct globals onto one cell, with nothing to reject it
    // (`validate` only bounds-checks). A widened slot would likewise keep its
    // stale declared `Ty` while the accessors moved to a register pair.
    fn compact_mutable_globals(sig: &mut SigInfer) -> Vec<(String, Ty)> {
        let mut mutable_globals: Vec<(String, Ty)> = Vec::new();
        sig.gvar_of.clear();
        for (slot, ty) in sig.global_tys.clone().into_iter().enumerate() {
            if let Some(ty) = ty {
                sig.gvar_of.insert(slot as u16, mutable_globals.len() as u32);
                let name = sig.global_names.get(slot).cloned().unwrap_or_default();
                mutable_globals.push((name, ty));
            }
        }
        mutable_globals
    }
    let mut mutable_globals = compact_mutable_globals(&mut sig);

    // Final pass with stable signatures: produce the real MIR + interned globals for
    // the reachable functions. Any reachable function outside the subset fails the
    // whole module (caller falls back) — unless `hybrid` is set and every failing
    // function is bridge-eligible, in which case the pass reruns with those
    // functions marked VM-executed (their call sites emit `CallVm`, their bodies
    // are skipped). `FuncId`s keep their original module indices.
    let final_pass = |sig: &mut SigInfer, reach: &[bool], funcs: &[FunctionData]| {
        // Same per-pass discipline as the fixpoint: `conflict` reflects *this*
        // pass only. A hybrid rerun re-derives every native function from
        // scratch, so a mark left by an earlier pass (a caller lowered
        // against a callee's stale ret assumptions before that callee was
        // VM-marked) must not survive into the post-loop check.
        sig.conflict = false;
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
                funcs,
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
    let (mut globals, mut functions, failures) = final_pass(&mut sig, &reachable, &funcs);
    // A bundled module exports more than any one importer uses, and those
    // extras are rooted speculatively — their names are reached by a lookup
    // the bytecode scan cannot follow, so there is no telling in advance which
    // are live. A dead one may still fail to lower (with no call site, its
    // parameter types were never observed), and failing the whole module for a
    // function nothing can call is the wrong answer. Drop those; anything the
    // emitted code still references is a real failure — as is anything
    // `#[export]`ed, which is referenced from *outside* the module, so nothing
    // in here names it.
    let referenced = referenced_functions(&functions);
    let speculative: std::collections::HashSet<usize> = bundles
        .iter()
        .flat_map(|b| b.fns.values().map(|&fidx| fidx as usize))
        .collect();
    let failures: Vec<(usize, Unsupported)> = failures
        .into_iter()
        .filter(|(fi, _)| {
            // Only a speculatively-rooted bundled function may be dropped, and
            // only when nothing reaches it: everything else that failed is
            // still a real failure, and in a hybrid build the failure list is
            // what decides which functions the VM bridge takes over.
            !speculative.contains(fi)
                || referenced.contains(&FuncId(*fi as u32))
                || funcs.get(*fi).is_some_and(|f| f.export_name.is_some())
        })
        .collect();
    if !failures.is_empty() {
        // `LK_AOT_DEBUG_FAILURES=1` lists every failing function (the
        // returned error is only the first; a callee's real blocker often
        // hides behind its caller's transient ret-type check).
        if std::env::var_os("LK_AOT_DEBUG_FAILURES").is_some() {
            for (fi, err) in &failures {
                eprintln!("lk-aot-lower: final-pass failure: fn{fi}: {err:?}");
            }
        }
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
        // Mark-and-rerun to a fixpoint. Two effects force the iteration:
        // (1) a caller — the entry included — can fail purely on the stale
        // ret assumptions of a callee that is now VM-executed; re-lowering
        // it against the bridge's `Dyn` result is the only sound judge.
        // (2) a first-pass failure can abort a caller *before* it reaches a
        // later call site, leaving that callee without the parameter
        // observations its eligibility needs — each rerun lets callers get
        // further and can surface new eligible callees. Every iteration
        // must mark at least one new function, so it runs ≤ funcs.len()
        // times; a failure that persists with nothing new to mark rejects
        // the module (the current Tier 0 behavior).
        let written = written_global_slots(&funcs);
        let mut current_failures = failures;
        loop {
            let mut marked_any = false;
            for (fi, _) in &current_failures {
                let eligible = bridge_eligibility(*fi, &funcs, module.entry, &sig, &written);
                if std::env::var_os("LK_AOT_DEBUG_FAILURES").is_some() {
                    // "why was this not bridged" is the usual question when a
                    // program unexpectedly falls back to Tier 0.
                    eprintln!("lk-aot-lower: fn{fi} failed to lower; bridge-eligible: {eligible:?}");
                }
                if !sig.vm_functions.contains_key(&(*fi as u32))
                    && let Some(param_count) = eligible
                {
                    sig.vm_functions.insert(*fi as u32, param_count);
                    marked_any = true;
                }
            }
            if !marked_any {
                return Err(current_failures
                    .first()
                    .map(|(_, err)| err.clone())
                    .unwrap_or(first_error));
            }
            let native_reachable = native_reachable_functions(&funcs, module.entry, &sig.vm_functions);
            // Drop VM marks without any native-reachable call site (a callee
            // only ever called from inside the VM needs no bridge signature).
            sig.vm_functions
                .retain(|fidx, _| native_reachable.get(*fidx as usize).copied().unwrap_or(false));
            // Re-converge before emitting: the marks just added changed what
            // the remaining native functions see. The global numbering is derived
            // from that same state, so it is rebuilt too.
            let mut retry_reachable = native_reachable.clone();
            refine_signatures(&mut sig, &mut funcs, &mut retry_reachable);
            let retry_mutable_globals = compact_mutable_globals(&mut sig);
            let (retry_globals, retry_functions, retry_failures) = final_pass(&mut sig, &retry_reachable, &funcs);
            if retry_failures.is_empty() {
                globals = retry_globals;
                functions = retry_functions;
                mutable_globals = retry_mutable_globals;
                break;
            }
            if std::env::var_os("LK_AOT_DEBUG_FAILURES").is_some() {
                for (fi, err) in &retry_failures {
                    eprintln!("lk-aot-lower: hybrid-rerun failure: fn{fi}: {err:?}");
                }
            }
            current_failures = retry_failures;
        }
    }
    if sig.conflict {
        return Err(Unsupported::ReturnTypeConflict);
    }
    let mut vm_functions: Vec<VmFunction> = sig
        .vm_functions
        .iter()
        .map(|(&fidx, &param_count)| VmFunction {
            id: FuncId(fidx),
            param_count,
        })
        .collect();
    vm_functions.sort_by_key(|vm_fn| vm_fn.id.0);
    let lowered = MirModule {
        abi_version: lk_aot_abi::ABI_VERSION,
        globals,
        mutable_globals,
        vm_functions,
        entry: FuncId(module.entry),
        functions,
    };
    // Totality guard (RFC §1): a successful `lower` must yield a *valid* module
    // so codegen never fails mid-emit. A Tier 1 hybrid rerun can re-lower a
    // caller against a now-bridged callee into structurally-invalid MIR (e.g. a
    // dangling entry block); catch that here and treat the module as
    // not-natively-lowerable rather than emitting it as an internal codegen
    // error. Debug the underlying shape with `LK_AOT_DEBUG_FAILURES=1`.
    if let Err(error) = lk_aot_mir::validate(&lowered) {
        if std::env::var_os("LK_AOT_DEBUG_FAILURES").is_some() {
            eprintln!("lk-aot-lower: lowered module failed validation: {error:?}");
        }
        return Err(Unsupported::InvalidMir);
    }
    Ok(lowered)
}

/// Every function id the emitted code can reach: direct calls, protected
/// calls, and function addresses taken as constants.
fn referenced_functions(functions: &[MirFunction]) -> std::collections::HashSet<FuncId> {
    let mut referenced = std::collections::HashSet::new();
    for function in functions {
        for block in &function.blocks {
            for inst in &block.insts {
                match inst {
                    Inst::CallFn { func, .. } | Inst::TryCall { func, .. } => {
                        referenced.insert(*func);
                    }
                    Inst::Const {
                        value: Const::FnAddr(func),
                        ..
                    } => {
                        referenced.insert(*func);
                    }
                    _ => {}
                }
            }
        }
    }
    referenced
}
