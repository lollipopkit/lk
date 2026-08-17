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
    /// `(function, TryBegin pc)` → the function that region's body became.
    ///
    /// Filled before any function is lowered, because a region's body has to
    /// exist as a function *before* the parent can call it — and because the
    /// bodies are ordinary entries in the function table from then on, lowered
    /// by the same loop as everything else.
    pub(crate) try_bodies: std::collections::HashMap<(u32, usize), u32>,
    /// A try body's parameters, as *registers of the enclosing function*.
    ///
    /// Discovered rather than declared: the body is lowered, and a read with no
    /// definition inside it names the register that has to come in from
    /// outside. Repeating that until it lowers gives exactly the set it needs —
    /// no table of which operand each opcode reads, which is the kind of table
    /// that is wrong in one entry and produces a wrong answer.
    pub(crate) try_body_params: std::collections::HashMap<u32, Vec<u8>>,
    /// Registers a body actually **rebound**, as opposed to objects it mutated
    /// through a handle it shares with the parent.
    ///
    /// Recorded while the body is lowered, by comparing the SSA's `current_def`
    /// before and after each instruction — the same device the body already
    /// uses to notice a cell's value changing, widened from the tracked cells
    /// to every register.
    ///
    /// It replaces reading the instruction's `a` field as "the register this
    /// writes", which is not true of every opcode: `log.push(2)` lowers to
    /// `ListPush a=log`, where `a` is the *receiver*. Counting that as a
    /// rebinding gave the register a cell, and the `dyn.from_list` /
    /// `dyn.as_list` round trip a cell implies is what loses the mutation.
    pub(crate) try_body_rebound: std::collections::HashMap<u32, std::collections::HashSet<u8>>,
    /// What type each of those inputs travels as, when it is not `I64`.
    ///
    /// The trampoline marshals a body's inputs as machine words in a stack
    /// buffer, so anything a word can hold may cross: an integer, and a
    /// container handle, which *is* a pointer. What may not are the carriers
    /// that occupy two registers (`Dyn`, the `Maybe`s) and `F64`, which the ABI
    /// passes in XMM while the trampoline passes integers.
    ///
    /// Recorded by the caller, which is where the register's real type is
    /// known, and read by the body on the next pass — the same fixpoint that
    /// discovers *which* registers are inputs at all.
    pub(crate) try_body_param_tys: std::collections::HashMap<(u32, u8), Ty>,
    /// Region inputs the enclosing function holds as a *closure reference*
    /// rather than as a value.
    ///
    /// A lambda has no runtime representation natively — it is a compile-time
    /// `GlobalRef`, which is why storing one in a list rejects — so a region
    /// input that is one has no word to marshal. It crosses the same way an
    /// erased lambda argument crosses an ordinary call instead: the *identity*
    /// travels at compile time (the body seeds the register with the ref) and
    /// only the environment travels at run time, as extra words in the same
    /// argument buffer.
    ///
    /// Without it `try { r = inner(); }` rejected for any local `inner`, which
    /// is a shape a `try` block is written around constantly.
    pub(crate) try_body_lambdas: std::collections::HashMap<(u32, u8), LambdaIdentity>,
    /// Region inputs the enclosing function holds as an *upvalue cell* — a
    /// variable some closure in it captured. See [`cell_region_input`].
    pub(crate) try_body_cell_inputs: std::collections::HashSet<(u32, u8)>,
    /// What a cell input's *content* type is, as the caller saw it entering the
    /// region.
    ///
    /// A cell is dynamically typed — reading one answers a `Dyn` — so without
    /// this every use of a captured variable inside a region became `Dyn`
    /// arithmetic, which has no lowering: `if (p0 % 5 == 0)` rejected for a `p0`
    /// some lambda in the function happened to capture. The body unboxes to this
    /// type instead, and a store of a *different* type joins the entry to `Dyn`
    /// and retries, so the two ends cannot disagree about what the cell holds.
    pub(crate) try_body_cell_input_tys: std::collections::HashMap<(u32, u8), Ty>,
    /// What a runtime-cell capture *holds*, by `(callee, capture index)`.
    ///
    /// A cell is dynamically typed, so reading one answers `Dyn` — and `Dyn`
    /// arithmetic has no lowering, so a closure that merely *adds* to what it
    /// captured rejected the moment the capture became a cell (which is what
    /// assigning to it, or handing it to a `try` region, does). The call site
    /// seeds the cell and therefore knows the type; the callee unboxes reads to
    /// it, and a store of a different type joins the entry to `Dyn` and retries,
    /// so the two ends cannot hold two opinions about one object.
    ///
    /// [`SigInfer::try_body_cell_input_tys`] is the same notion for a region
    /// input, keyed by *register* because that is what the caller has there.
    pub(crate) cell_capture_tys: std::collections::HashMap<(u32, usize), Ty>,
    /// Lambdas the program uses as **runtime values** — stored in a container,
    /// put in a struct field, returned from a branch — mapped to the *clone*
    /// that is that value.
    ///
    /// A clone, not the lambda itself. A closure value is called through one
    /// arity switch in the runtime, so it must have an all-`Dyn` signature;
    /// the same lambda's other uses are often the ones that resolve statically,
    /// and the typed HOF path takes its address with the typed signature.
    /// Pinning the original to `Dyn` cost `examples/syntax/closure.lk` its
    /// lowering. So the original keeps its signature and the value form is a
    /// second copy of the body — the mechanism lambda erasure already uses.
    ///
    /// The value is built **at the consumer that needs one**
    /// (`lower_call::read_value`), never at the definition, so a register that
    /// names a lambda keeps exactly one meaning and `Move`, a call window and
    /// an iteration need to know nothing about any of this.
    pub(crate) value_lambdas: std::collections::HashMap<u32, u32>,
    /// The clones themselves: what [`SigInfer::param_ty`] answers `Dyn` for.
    pub(crate) value_lambda_bodies: std::collections::HashSet<u32>,
    /// What type each of those environment words travels as, keyed by
    /// `(body, register, capture index)` — the [`SigInfer::try_body_param_tys`]
    /// of a lambda input, which needs one type per capture rather than one per
    /// register.
    pub(crate) try_body_lambda_env_tys: std::collections::HashMap<(u32, u8, u8), Ty>,
    /// A try body's *outputs*: registers of the enclosing function that the
    /// body assigns and the enclosing function goes on to read.
    ///
    /// They cannot travel in registers. The body runs in a frame of its own, so
    /// a write there leaves the parent's copy alone — and on the raise path the
    /// body never returns at all, while the VM still shows whatever it managed
    /// to write. So each one becomes a cell: the parent makes it, the body
    /// writes through it as it goes, and the parent reads it back on both
    /// edges.
    pub(crate) try_body_cells: std::collections::HashMap<u32, Vec<u8>>,
    /// Which of a body's cells the *caller* allocated as **raw** — parking a
    /// typed container handle rather than a boxed value.
    ///
    /// The kind is one decision, and it belongs to whoever creates the cell.
    /// Both sides used to decide it independently — the caller from the
    /// register's type *entering* the region, the body from the type it
    /// *stores* — and the two disagree exactly when a register that was `nil`
    /// is assigned a container inside the body. `let out = nil; try { out =
    /// b.take(1); } catch e { }` then wrote a raw handle into a value cell, and
    /// the read raised "runtime type error" where the VM printed the bytes.
    pub(crate) try_body_raw_cells: std::collections::HashSet<(u32, u8)>,
    /// Registers a *later* read proved the body had to write back.
    ///
    /// `try_body_cells` is what the region's own scan could see: registers the
    /// enclosing function had already defined. This is the other half — a
    /// register first defined *inside* the body and read after it, which the
    /// scan cannot know about because nothing in the parent defines it. The
    /// read itself is the evidence, and it arrives as an `UndefinedOperand`.
    pub(crate) try_body_extra_cells: std::collections::HashMap<u32, std::collections::HashSet<u8>>,
    /// Try bodies that `return` from the **enclosing** function.
    ///
    /// A body is outlined into a function of its own, so a `return` written in
    /// it would return from *that* function — a different program. It used to be
    /// refused, which made `try { return n * 2; } catch e { return -1; }` drop
    /// the whole program to the VM while the value form
    /// (`let v = try { n * 2 } catch e { -1 }; return v;`) lowered. The same
    /// function, two spellings, one of them three times slower.
    ///
    /// So the body gets a third channel beside "the value" and "it raised": two
    /// more output cells, a flag and the value. The body sets them and returns
    /// normally; the caller checks the flag on the ok edge and returns.
    pub(crate) try_body_returns: std::collections::HashSet<u32>,
    /// Empty-`[]` literals whose guessed element type a consumer
    /// contradicted (`(function, pc)`): the next fixpoint pass materializes
    /// them as Dyn lists.
    pub(crate) dyn_literals: std::collections::HashSet<(u32, usize)>,
    /// `(function, parameter register)` pairs whose list argument must be
    /// built as a Dyn list by every caller, because the callee pushes an
    /// element the typed carrier cannot hold.
    ///
    /// The demand travels *up*: a callee cannot fix its own parameter (the
    /// allocation belongs to the caller, and the caller's other aliases read
    /// it), so the carrier has to be decided at the literal.
    pub(crate) dyn_params: std::collections::HashSet<(u32, u8)>,
    /// Loop-header phis discovered to merge heterogeneous boxable types
    /// (`(function, block, slot)`): the next fixpoint pass pre-types them
    /// `Dyn` so the loop body consumes them through the Dyn arms.
    pub(crate) dyn_loop_phis: std::collections::HashSet<(u32, usize, usize)>,
    /// Functions whose returns disagreed on a boxable type (or returned a
    /// nullable carrier): the next fixpoint pass boxes every return point,
    /// making the function return `Dyn` instead of rejecting the module.
    pub(crate) dyn_rets: std::collections::HashSet<u32>,
    /// `(function, capture index)` pairs that must travel as a **runtime cell**
    /// rather than by value, because the body assigns to them.
    ///
    /// A closure's captures are hidden trailing arguments holding the cell's
    /// content at the call site — right for a capture the body reads, and with
    /// nowhere to put a write. So `|v| { acc = acc + v; }`, which is most of
    /// what a closure is for, dropped the whole program to the VM.
    ///
    /// Discovered the same way `dyn_rets` and `try_body_params` are: the body
    /// is lowered, the assignment finds a by-value capture, records the pair
    /// and asks for a retry. The next pass has the caller seed an `rt.cell_new`
    /// and read it back — the same carrier a `try` body's outer assignment
    /// already crosses on. Nothing guesses at the bytecode's register
    /// provenance, and a read-only capture keeps passing as a plain value.
    pub(crate) cell_captures: std::collections::HashSet<(u32, usize)>,
    /// `(function, capture index)` → the callable that capture *is*.
    ///
    /// `let f = |x| x + 1; let g = |x| f(x) * 2;` — composing two lambdas, which
    /// is most of what having them is for. `f` is captured, so the compiler puts
    /// it in a cell, and what goes into that cell is a lowering-time reference,
    /// not a value. The callee's `LoadCapture` + `LoadCellVal` then read a
    /// parameter that holds nothing meaningful.
    ///
    /// A reference has no runtime representation here, so the capture still
    /// occupies its ABI slot (a dead `0`) and the *meaning* travels through this
    /// map instead. Discovered by the caller and retried, the same loop
    /// `cell_captures` uses — so the callee never lowers before the fact exists;
    /// if it somehow did, the `Call` on a plain integer refuses and the retry
    /// fixes it.
    pub(crate) ref_captures: std::collections::HashMap<(u32, usize), GlobalRef>,
    /// Per function: the struct its returns are known to construct.
    ///
    /// A type's *name* only ever entered the lowering from a `NewObject`
    /// (`ssa.struct_types`), so it stopped at the function boundary: the
    /// receiver of `make(3, 4).norm()` had no type and the method call fell out
    /// of the devirtualizing path — in one module as much as across two. This
    /// carries it out, and the fixpoint carries it to callers lowered before
    /// their callee.
    ///
    /// `Some(None)` where the returns disagree or one of them is not a struct:
    /// an answer that is sometimes wrong would devirtualize to the wrong impl.
    pub(crate) ret_structs: std::collections::HashMap<u32, Option<String>>,
    /// `(callee, parameter slot)` → the struct every call site passes there.
    ///
    /// The parameter-side twin of [`Self::ret_structs`], and the same missing
    /// provenance one step earlier: a struct arriving as an *argument* had no
    /// type name, so `fn area(q: P) { return q.w * q.h; }` read fields fine
    /// (the carrier is `MapStrDyn` either way) while `fn area(q: P) { return
    /// q.norm(); }` could not devirtualize and dropped the module to the VM.
    /// Passing a value to a function is at least as common as returning one.
    ///
    /// `Some(None)` where the call sites disagree, or one of them passes
    /// something that is not a struct: a name that is right only sometimes
    /// would devirtualize to the wrong impl, which is worse than not lowering.
    /// [`Self::observe_param`] takes the argument's name as a parameter — not
    /// as a separate call the caller might forget — because a site that
    /// silently records nothing inherits another site's answer, and that is
    /// exactly the wrong-impl case.
    pub(crate) param_structs: std::collections::HashMap<(usize, usize), Option<String>>,
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
    /// its hybrid retry (`docs/aot/tier1-hybrid.md`).
    pub(crate) vm_functions: std::collections::HashMap<u32, usize>,
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
    /// Appends one function's worth of state to **every** per-function table,
    /// returning its index.
    ///
    /// These tables are parallel arrays indexed by function, and the working
    /// function list grows in three places: `try`-body outlining, a
    /// lambda-argument specialization, and a closure-value clone. Each pushed
    /// to the subset it happened to care about, and the subsets differed — so
    /// after a single outlined `try` body, `lambda_params.len()` was one short
    /// of `param_obs.len()` and a specialization's entry landed under the
    /// *previous* function's index. The visible symptom was that
    /// `fn ap(xs, f) { return xs.map(f); }` stopped lowering as soon as the
    /// module contained a `try` anywhere, because the erased lambda parameter
    /// was recorded for somebody else.
    pub(crate) fn push_function(&mut self, params: Vec<Option<Ty>>, ret: Ty) -> u32 {
        let index = self.param_obs.len() as u32;
        self.param_obs.push(params);
        self.ret_types.push(ret);
        self.ret_known.push(true);
        self.lambda_params.push(Vec::new());
        self.specialized.push(false);
        self.plain_called.push(false);
        self.ret_closures.push(None);
        self.ret_closure_poisoned.push(false);
        debug_assert!(
            [
                self.ret_types.len(),
                self.ret_known.len(),
                self.lambda_params.len(),
                self.specialized.len(),
                self.plain_called.len(),
                self.ret_closures.len(),
                self.ret_closure_poisoned.len(),
            ]
            .iter()
            .all(|&len| len == self.param_obs.len()),
            "per-function tables must stay parallel"
        );
        index
    }

    /// The type a parameter is believed to hold.
    ///
    /// An unobserved parameter defaults to `I64` rather than `Dyn`. `Dyn`
    /// looks like the honest answer for a function nothing calls, but such a
    /// function still *makes* calls, and a `Dyn` argument at one of those is
    /// recorded as an observation — so a dead export widens the parameters of
    /// the live functions it happens to call. A function that cannot lower on
    /// the `I64` guess is dropped instead, provided nothing reaches it.
    pub(crate) fn param_ty(&self, func: usize, i: usize) -> Ty {
        // The value form of a lambda is called through one arity switch, so
        // every one of them has the same signature: all `Dyn`, parameters and
        // captures alike. Same pinning `spawn` does to the body it launches by
        // address.
        if self.value_lambda_bodies.contains(&(func as u32)) {
            return Ty::Dyn;
        }
        if let Some(observed) = self.param_obs[func].get(i).copied().flatten() {
            return observed;
        }
        // `self` in `impl T { … }` is a struct instance, whatever the call
        // sites said — including when there are none. Every impl method is a
        // lowering root (a trait's arms must all exist), so an *uncalled* one
        // was lowered with the `I64` default and then failed reading a field:
        // `an operand at pc 1 is a str where a i64 is required`, in a method
        // nobody calls, killing the whole module. `t4`/`t6` in the trait notes
        // are exactly that.
        if i == 0 && self.traits.impl_owner(func as u32).is_some() {
            return Ty::MapStrDyn;
        }
        Ty::I64
    }

    /// Records one call-site observation of `callee`'s parameter `slot_idx`
    /// and returns the parameter's (possibly widened) type. Disagreeing
    /// observations join to `Dyn` — the parameter becomes dynamically typed
    /// and every call site boxes — instead of rejecting the module. Nullable
    /// shapes (`Nil`, the `Maybe` carriers) have no typed parameter form and
    /// observe as `Dyn` directly. The join is monotonic on a two-level
    /// lattice, so the fixpoint still terminates; function-vs-value
    /// polymorphism keeps its own reject (`lambda_params`).
    pub(crate) fn observe_param(&mut self, callee: usize, slot_idx: usize, arg_ty: Ty, arg_struct: Option<&str>) -> Ty {
        match self.param_structs.entry((callee, slot_idx)) {
            std::collections::hash_map::Entry::Vacant(slot) => {
                slot.insert(arg_struct.map(str::to_string));
            }
            std::collections::hash_map::Entry::Occupied(mut slot) => {
                if slot.get().as_deref() != arg_struct {
                    slot.insert(None);
                }
            }
        }
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

    /// Whether *every* capture of `callee` is a static reference.
    ///
    /// Then the closure needs nothing at runtime — it is a plain function
    /// reference — so the capture environment is erased entirely rather than
    /// carried as dead slots. That is what lets `xs.map(|x| f(x))` reach the
    /// typed `map_fn` fast path, which calls the callback with exactly the
    /// element and nothing else.
    ///
    /// All-or-nothing on purpose: a *mixed* environment would need a hole at one
    /// index, and every call site would have to agree on where the hole is. The
    /// dead-slot form already handles that case correctly, just with one wasted
    /// register.
    pub(crate) fn captures_all_static(&self, callee: usize, capture_count: usize) -> bool {
        capture_count > 0 && (0..capture_count).all(|k| self.ref_captures.contains_key(&(callee as u32, k)))
    }

    /// Records that capture `k` of `callee` has to arrive as a runtime cell,
    /// and **pins** its parameter slot to [`Ty::Cell`].
    ///
    /// The pin is the point: `param_obs` accumulates across fixpoint passes and
    /// never resets, so the by-value type observed before the body's assignment
    /// was seen would join with `Cell` to `Dyn` and the call site would then
    /// fail to coerce the cell pointer at all. Returns whether this is new
    /// information (the caller retries when it is).
    pub(crate) fn require_cell_capture(&mut self, callee: usize, param_count: usize, k: usize) -> bool {
        let fresh = self.cell_captures.insert((callee as u32, k));
        if let Some(slot) = self.param_obs.get_mut(callee).and_then(|p| p.get_mut(param_count + k)) {
            let changed = *slot != Some(Ty::Cell);
            *slot = Some(Ty::Cell);
            return fresh || changed;
        }
        fresh
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
            // A capture taken onward from an enclosing closure is not one of
            // *this* function's parameter values, so the summary does not apply.
            ClosureCapture::CellParam(_) | ClosureCapture::StaticRef => return None,
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
