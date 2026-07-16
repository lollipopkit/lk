use super::*;

/// Branch args this block passes to `target`: one per target phi, taken from the
/// operand that phi recorded for the `from` predecessor.
pub(crate) fn args_to(ssa: &Ssa, from: usize, target: usize) -> Vec<ValueId> {
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

pub(crate) fn build_term(
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
        // `ret_val` is `None` for a resolved `Nil` return value (`ret void`)
        // and `Some` for a bare `return` in a Dyn-returning function (nil
        // crosses the call boundary boxed).
        Some(Exit::Ret(None)) | Some(Exit::Ret(Some(_))) => Term::Ret(ret_val),
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
pub(crate) struct Phi {
    pub(crate) param: ValueId,
    pub(crate) reg: usize,
    pub(crate) ty: Ty,
    pub(crate) operands: Vec<(usize, ValueId)>,
}

pub(crate) struct Ssa {
    pub(crate) reg_count: usize,
    /// Register slots plus the virtual cell slots appended after them
    /// (`reg_count + cid` addresses cell `cid`'s content).
    pub(crate) slot_count: usize,
    pub(crate) capture_slots: usize,
    /// This function runs as a spawned goroutine (isolate): cell-capture
    /// writes go to the thread-private slots.
    pub(crate) spawned_isolate: bool,
    pub(crate) preds: Vec<Vec<usize>>,
    pub(crate) current_def: Vec<Vec<Option<Reg>>>,
    pub(crate) sealed: Vec<bool>,
    pub(crate) filled: Vec<bool>,
    pub(crate) phis: Vec<Vec<Phi>>,
    pub(crate) incomplete: Vec<Vec<usize>>,
    /// For fallthrough (`None` exit) blocks, the sole successor block's leader pc.
    pub(crate) single_fallthrough_target: Vec<Option<usize>>,
    pub(crate) next_val: u32,
    /// Compile-time-known values, for provably-in-bounds constant list indexing:
    /// SSA value → its constant `i64` (recorded for direct `LoadInt`s).
    pub(crate) const_int: std::collections::HashMap<ValueId, i64>,
    /// Loop-header phis pre-typed `Dyn` by a fixpoint retry (slots keyed by
    /// `(block, slot)`; see `Unsupported::DynLoopPhi`).
    pub(crate) dyn_loop_slots: std::collections::HashSet<(usize, usize)>,
    /// Empty-`[]` literal pcs forced to Dyn by a fixpoint retry.
    pub(crate) dyn_empty_pcs: std::collections::HashSet<usize>,
    /// Guessed empty-list handles → their literal pc (a consumer that
    /// contradicts the guess reports `EmptyListGuessWrong`).
    pub(crate) empty_guess: std::collections::HashMap<ValueId, (usize, Ty)>,
    /// Constant-range materializations (`NewRange` with all-const operands,
    /// step 1): handle → exclusive `(start, end)`. Lets `GetIndex` recognize
    /// a range key (`s[1..3]`) and emit a real slice.
    pub(crate) range_def: std::collections::HashMap<ValueId, (i64, i64)>,
    /// SSA value of a const-materialized list handle → its known element count.
    pub(crate) list_len: std::collections::HashMap<ValueId, i64>,
    /// Element count at materialization (never bumped by pushes): a sound
    /// *lower bound* on the runtime length — the subset has no removal ops,
    /// while `list_len`'s static push increments can overshoot (a push in an
    /// untaken branch). Used to prove both operands of a cross-typed list
    /// comparison non-empty.
    pub(crate) list_base_len: std::collections::HashMap<ValueId, i64>,
    /// SSA value → its compile-time string content (`LoadString` /
    /// `LoadHeapConst` long strings), used to expand `println` format strings
    /// at lower time.
    pub(crate) const_strs: std::collections::HashMap<ValueId, String>,
    /// `(block, register)` → global reference (runtime builtin, module object,
    /// or resolved module function) loaded there by `GetGlobal`/`GetIndex` and
    /// propagated by `Move`. Block-local by construction; any write to the
    /// register clears it.
    pub(crate) builtin_regs: std::collections::HashMap<(usize, u8), GlobalRef>,
    /// Fresh ids for upvalue cells created by `LoadHeapConst`; each cell's
    /// content lives in virtual slot `reg_count + cid`, participating in the
    /// same Braun construction as registers (cross-block cell state gets
    /// phis). Iteration isolation needs no extra guard: the only path to a
    /// cell read is a `Cell`/`Closure` ref in `builtin_regs`, and ref
    /// propagation dies at loop headers whose entry edge lacks the ref, while
    /// the creation site re-initializes the slot each iteration.
    pub(crate) next_cell: u32,
    /// Per-block trailing instructions added for phi-edge type conversions
    /// (`Maybe` ↔ scalar merges); appended after the block's own instructions
    /// when the MIR blocks are assembled.
    pub(crate) edge_insts: Vec<Vec<Inst>>,
    /// `NewObject` provenance: the struct type name behind a `MapStrDyn`
    /// handle value (plan J1). Method calls and display contexts consult the
    /// trait table through it; `Move` preserves the `ValueId`, so the entry
    /// follows the value across registers for free.
    pub(crate) struct_types: std::collections::HashMap<ValueId, String>,
}

impl Ssa {
    pub(crate) fn new(
        reg_count: usize,
        cell_capacity: usize,
        capture_count: usize,
        preds: Vec<Vec<usize>>,
        total_blocks: usize,
    ) -> Self {
        // Virtual slot layout: registers, then `UpvalCell` cells, then one
        // slot per capture parameter (a spawned goroutine's thread-private
        // cell copies — isolate semantics).
        let slot_count = reg_count + cell_capacity + capture_count;
        Self {
            reg_count,
            slot_count,
            capture_slots: capture_count,
            spawned_isolate: false,
            preds,
            current_def: vec![vec![None; slot_count]; total_blocks],
            sealed: vec![false; total_blocks],
            filled: vec![false; total_blocks],
            phis: (0..total_blocks).map(|_| Vec::new()).collect(),
            incomplete: (0..total_blocks).map(|_| Vec::new()).collect(),
            single_fallthrough_target: vec![None; total_blocks],
            next_val: 0,
            const_int: std::collections::HashMap::new(),
            dyn_loop_slots: std::collections::HashSet::new(),
            dyn_empty_pcs: std::collections::HashSet::new(),
            empty_guess: std::collections::HashMap::new(),
            range_def: std::collections::HashMap::new(),
            list_len: std::collections::HashMap::new(),
            list_base_len: std::collections::HashMap::new(),
            const_strs: std::collections::HashMap::new(),
            builtin_regs: std::collections::HashMap::new(),
            next_cell: 0,
            edge_insts: vec![Vec::new(); total_blocks],
            struct_types: std::collections::HashMap::new(),
        }
    }

    pub(crate) fn new_val(&mut self) -> ValueId {
        let v = ValueId(self.next_val);
        self.next_val += 1;
        v
    }

    pub(crate) fn write(&mut self, reg: u8, block: usize, value: Reg) {
        if (reg as usize) < self.reg_count {
            self.write_slot(reg as usize, block, value);
        }
    }

    pub(crate) fn write_slot(&mut self, slot: usize, block: usize, value: Reg) {
        if slot < self.slot_count {
            self.current_def[block][slot] = Some(value);
            if slot < self.reg_count {
                self.builtin_regs.remove(&(block, slot as u8));
            }
        }
    }

    pub(crate) fn read(&mut self, reg: u8, block: usize, pc: usize) -> Result<Reg, Unsupported> {
        self.read_slot(reg as usize, block, pc)
    }

    pub(crate) fn read_slot(&mut self, slot: usize, block: usize, pc: usize) -> Result<Reg, Unsupported> {
        if let Some(v) = self.current_def[block][slot] {
            return Ok(v);
        }
        self.read_recursive(slot, block, pc)
    }

    /// The virtual slot holding cell `cid`'s content.
    pub(crate) fn cell_slot(&self, cid: u32) -> usize {
        self.reg_count + cid as usize
    }

    /// The thread-private slot backing capture parameter `k` in a spawned
    /// goroutine (seeded from the boxed snapshot at entry).
    pub(crate) fn cellparam_slot(&self, k: usize) -> usize {
        self.slot_count - self.capture_slots + k
    }

    pub(crate) fn read_typed(&mut self, reg: u8, block: usize, want: Ty, pc: usize) -> Result<ValueId, Unsupported> {
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
    pub(crate) fn builtin_ref_at(&self, reg: u8, block: usize) -> Option<GlobalRef> {
        let mut visited = std::collections::HashSet::new();
        let mut found: Option<GlobalRef> = None;
        if self.collect_builtin_ref(reg, block, &mut visited, &mut found) {
            found
        } else {
            None
        }
    }

    pub(crate) fn collect_builtin_ref(
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

    pub(crate) fn reg_const_str(&self, reg: u8, block: usize) -> Option<String> {
        let mut visited = std::collections::HashSet::new();
        let mut found: Option<String> = None;
        if self.collect_reg_const_str(reg as usize, block, &mut visited, &mut found) {
            found
        } else {
            None
        }
    }

    pub(crate) fn collect_reg_const_str(
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
    pub(crate) fn phi_ty(&mut self, slot: usize, block: usize, pc: usize) -> Result<Ty, Unsupported> {
        let preds = self.preds[block].clone();
        for p in preds {
            if self.filled[p] {
                return Ok(self.read_slot(slot, p, pc)?.1);
            }
        }
        Err(Unsupported::UndefinedOperand { pc, reg: slot })
    }

    pub(crate) fn read_recursive(&mut self, slot: usize, block: usize, pc: usize) -> Result<Reg, Unsupported> {
        let value: Reg = if !self.sealed[block] {
            let ty = self.phi_ty(slot, block, pc)?;
            // A fixpoint retry pre-types a discovered heterogeneous
            // loop-header phi as Dyn (the body then consumes it uniformly).
            let ty = if self.dyn_loop_slots.contains(&(block, slot)) {
                Ty::Dyn
            } else {
                ty
            };
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
            let ty = if self.dyn_loop_slots.contains(&(block, slot)) {
                Ty::Dyn
            } else {
                ty
            };
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
            // A heterogeneous merge may have widened the phi to Dyn.
            (param, self.phis[block][idx].ty)
        };
        self.current_def[block][slot] = Some(value);
        Ok(value)
    }

    pub(crate) fn add_phi_operands(&mut self, block: usize, phi_idx: usize, pc: usize) -> Result<(), Unsupported> {
        let slot = self.phis[block][phi_idx].reg;
        let phi_ty = self.phis[block][phi_idx].ty;
        let param = self.phis[block][phi_idx].param;
        let preds = self.preds[block].clone();
        let mut incoming = Vec::with_capacity(preds.len());
        for p in preds {
            let (v, ty) = self.read_slot(slot, p, pc)?;
            incoming.push((p, v, ty));
        }
        // Every incoming edge must agree on the type (the phi was typed from
        // one filled predecessor). A `Maybe` merging with its scalar (the
        // `let v = m[k]; if v == nil { v = default; }` shape) converts on
        // the incoming edge: extracting the raw value never observes the
        // absent case (the phi takes the other edge there), and wrapping a
        // scalar marks it present.
        let maybe_pair = |from: Ty, to: Ty| {
            matches!(
                (from, to),
                (Ty::MaybeI64, Ty::I64)
                    | (Ty::MaybeF64, Ty::F64)
                    | (Ty::MaybeStr, Ty::Str)
                    | (Ty::I64, Ty::MaybeI64)
                    | (Ty::F64, Ty::MaybeF64)
                    | (Ty::Str, Ty::MaybeStr)
            )
        };
        // A Nil-typed phi has no LLVM value form (`phi void`): even a
        // homogeneous all-nil merge widens to Dyn below (each edge boxes
        // `from_nil`; consumers test the tag at runtime, VM-exact).
        if phi_ty != Ty::Nil
            && incoming
                .iter()
                .all(|&(_, _, ty)| ty == phi_ty || maybe_pair(ty, phi_ty))
        {
            // A guessed empty-`[]` handle read through a phi (loop/branch)
            // keeps its provenance: every non-self edge must carry the same
            // literal pc for the param to inherit it.
            let mut guess: Option<(usize, Ty)> = None;
            let mut all_guessed = true;
            for &(_, v, _) in &incoming {
                if v == param {
                    continue;
                }
                match self.empty_guess.get(&v) {
                    Some(&g) if guess.is_none() || guess == Some(g) => guess = Some(g),
                    _ => {
                        all_guessed = false;
                        break;
                    }
                }
            }
            if all_guessed && let Some(g) = guess {
                self.empty_guess.insert(param, g);
            }
            for (p, v, ty) in incoming {
                let v = if ty == phi_ty {
                    v
                } else {
                    self.convert_phi_edge(v, ty, phi_ty, p)
                        .expect("maybe_pair-checked edge converts")
                };
                self.phis[block][phi_idx].operands.push((p, v));
            }
            return Ok(());
        }
        // A heterogeneous merge of dyn-boxable types (`a ?? "default"`,
        // `if c { 1 } else { "x" }`) widens the phi to `Dyn` and boxes each
        // edge (plan M4.2). Only for a freshly created forward-join phi:
        // a loop-header phi's param may already be consumed at its old type
        // elsewhere (`allow_widen` = false from `seal_block`), and a
        // self-referential edge would box the param into itself.
        let boxable = |ty: Ty| {
            matches!(
                ty,
                Ty::Dyn
                    | Ty::Nil
                    | Ty::Bool
                    | Ty::I64
                    | Ty::F64
                    | Ty::Str
                    | Ty::ListDyn
                    | Ty::ListI64
                    | Ty::ListF64
                    | Ty::ListStr
                    | Ty::MapStrDyn
                    | Ty::MaybeI64
                    | Ty::MaybeF64
                    | Ty::MaybeStr
                    | Ty::MaybeBool
            )
        };
        if phi_ty != Ty::Dyn || incoming.iter().any(|&(_, v, ty)| v == param || !boxable(ty)) {
            // A boxable heterogeneous merge is retriable: report it so the
            // fixpoint re-lowers with this phi pre-typed `Dyn` *from
            // creation*. Widening in place is unsound even for a fresh
            // forward join — a same-pass consumer (an inner if-join phi over
            // the same slot) may already have read the param at its old type
            // and boxed the Dyn value as if it were that type.
            if phi_ty != Ty::Dyn && incoming.iter().all(|&(_, v, ty)| v != param && boxable(ty)) {
                return Err(Unsupported::DynLoopPhi { block, slot });
            }
            return Err(Unsupported::TypeMismatch { pc });
        }
        for (p, v, ty) in incoming {
            let boxed = self.dyn_box_on_edge(p, v, ty).expect("boxable-checked edge boxes");
            self.phis[block][phi_idx].operands.push((p, boxed));
        }
        Ok(())
    }

    /// Emits the `dyn.from_*` boxing sequence for one phi edge into
    /// `edge_insts[pred]` (they land after the block body, before the
    /// terminator). Mirrors `to_dyn`, but targets an edge, not the body.
    pub(crate) fn dyn_box_on_edge(&mut self, pred: usize, v: ValueId, ty: Ty) -> Option<ValueId> {
        let simple = match ty {
            Ty::Dyn => return Some(v),
            Ty::I64 => Some("from_i64"),
            Ty::F64 => Some("from_f64"),
            Ty::Str => Some("from_str"),
            Ty::ListDyn => Some("from_list"),
            Ty::MapStrDyn => Some("from_map"),
            _ => None,
        };
        if let Some(name) = simple {
            let dst = self.new_val();
            self.edge_insts[pred].push(Inst::Call {
                dst: Some(dst),
                callee: AbiRef::new("dyn", name),
                args: vec![v],
            });
            return Some(dst);
        }
        match ty {
            Ty::Nil => {
                let dst = self.new_val();
                self.edge_insts[pred].push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "from_nil"),
                    args: Vec::new(),
                });
                Some(dst)
            }
            Ty::Bool => {
                let wide = self.new_val();
                self.edge_insts[pred].push(Inst::ZextBool { dst: wide, src: v });
                let dst = self.new_val();
                self.edge_insts[pred].push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "from_bool"),
                    args: vec![wide],
                });
                Some(dst)
            }
            // Nullable carriers box via `(value, present)` — a nil edge
            // arrives as nil, exactly the VM (`m[k]!` join shapes).
            Ty::MaybeI64 | Ty::MaybeF64 | Ty::MaybeStr | Ty::MaybeBool => {
                let from = match ty {
                    Ty::MaybeI64 => "from_maybe_i64",
                    Ty::MaybeF64 => "from_maybe_f64",
                    Ty::MaybeStr => "from_maybe_str",
                    _ => "from_maybe_bool",
                };
                let value = self.new_val();
                self.edge_insts[pred].push(Inst::MaybeValue {
                    dst: value,
                    src: v,
                    maybe_ty: ty,
                });
                let present_b = self.new_val();
                self.edge_insts[pred].push(Inst::MaybePresent {
                    dst: present_b,
                    src: v,
                    maybe_ty: ty,
                });
                let present = self.new_val();
                self.edge_insts[pred].push(Inst::ZextBool {
                    dst: present,
                    src: present_b,
                });
                let dst = self.new_val();
                self.edge_insts[pred].push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", from),
                    args: vec![value, present],
                });
                Some(dst)
            }
            Ty::ListI64 | Ty::ListF64 | Ty::ListStr => {
                let converter = match ty {
                    Ty::ListI64 => "i64_to_dyn",
                    Ty::ListF64 => "f64_to_dyn",
                    _ => "str_to_dyn",
                };
                let converted = self.new_val();
                self.edge_insts[pred].push(Inst::Call {
                    dst: Some(converted),
                    callee: AbiRef::new("list_h", converter),
                    args: vec![v],
                });
                let dst = self.new_val();
                self.edge_insts[pred].push(Inst::Call {
                    dst: Some(dst),
                    callee: AbiRef::new("dyn", "from_list"),
                    args: vec![converted],
                });
                Some(dst)
            }
            _ => None,
        }
    }

    pub(crate) fn convert_phi_edge(&mut self, v: ValueId, from: Ty, to: Ty, pred: usize) -> Option<ValueId> {
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

    pub(crate) fn mark_filled(&mut self, block: usize) {
        self.filled[block] = true;
    }

    /// Seals every unsealed block whose predecessors are all filled (a fixpoint,
    /// so sealing a loop header after its back-edge predecessor fills takes effect).
    pub(crate) fn seal_ready(&mut self) -> Result<(), Unsupported> {
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

    pub(crate) fn seal_block(&mut self, block: usize) -> Result<(), Unsupported> {
        let incs = std::mem::take(&mut self.incomplete[block]);
        for idx in incs {
            // No Dyn widening here: an incomplete phi (loop header) has
            // already been read at its original type inside the loop body.
            self.add_phi_operands(block, idx, 0)?;
        }
        self.sealed[block] = true;
        Ok(())
    }
}
