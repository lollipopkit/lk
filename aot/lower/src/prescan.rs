use super::*;

/// Reachability from the entry over `CallDirect`/`MakeClosure` edges that does
/// **not** descend into VM-executed functions: their bodies (and everything
/// only they reach) run on the embedded VM, so no native lowering is needed.
/// VM-executed functions themselves stay marked (they need native call sites).
pub(crate) fn native_reachable_functions(
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
pub(crate) fn written_global_slots(funcs: &[FunctionData]) -> std::collections::HashSet<u16> {
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
pub(crate) fn bridge_eligibility(
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
pub(crate) fn prescan_lambda_globals(module: &lk_core::vm::ModuleData, global_count: usize) -> Vec<Option<u32>> {
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

pub(crate) fn prescan_initialized_globals(module: &lk_core::vm::ModuleData, global_count: usize) -> Vec<bool> {
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
pub(crate) fn reachable_functions(module: &lk_core::vm::ModuleData, extra_roots: &[usize]) -> Vec<bool> {
    let n = module.functions.len();
    let mut reachable = vec![false; n];
    let entry = module.entry as usize;
    if entry >= n {
        return reachable;
    }
    let mut stack = vec![entry];
    reachable[entry] = true;
    for &root in extra_roots {
        if root < n && !reachable[root] {
            reachable[root] = true;
            stack.push(root);
        }
    }
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
pub(crate) fn empty_map_is_int_keyed(func: &FunctionData, start_pc: usize, dst_reg: u8) -> bool {
    let code = &func.code;
    let mut regs = std::collections::HashSet::new();
    // Registers most recently written by a string producer: an index through
    // one of these means the map is string-keyed. A wrong guess only costs a
    // fallback (the typed lowering rejects the mismatch), never a miscompile.
    let mut str_regs = std::collections::HashSet::new();
    // Registers holding a string *list* (a constant list of strings, a
    // `split` result): an element read out of one is a string key producer
    // (`freq[words[i]]`).
    let mut strlist_regs = std::collections::HashSet::new();
    // Producers are tracked from the top of the function (a key's string-ness
    // is often established *before* the map literal — `text.split` feeding a
    // later `freq[word]`); the map-register set only exists from the literal.
    for (pc, raw) in code.iter().enumerate() {
        let Ok(instr) = Instr::try_from_raw(*raw) else { break };
        if pc == start_pc {
            regs.insert(dst_reg);
            continue;
        }
        let after = pc > start_pc;
        match instr.opcode() {
            // A move re-types the destination: membership follows the
            // *source* in every set (a tracked register overwritten by an
            // unrelated value must drop out, or a later index through it
            // mis-attributes — the `regs` map-set is the dangerous one).
            Opcode::Move => {
                let b = instr.b();
                let a = instr.a();
                if regs.contains(&b) {
                    regs.insert(a);
                } else {
                    regs.remove(&a);
                }
                if strlist_regs.contains(&b) {
                    strlist_regs.insert(a);
                } else {
                    strlist_regs.remove(&a);
                }
                if str_regs.contains(&b) {
                    str_regs.insert(a);
                } else {
                    str_regs.remove(&a);
                }
            }
            Opcode::StringSplit => {
                strlist_regs.insert(instr.a());
                regs.remove(&instr.a());
                str_regs.remove(&instr.a());
            }
            // Iteration normalization keeps the element kind.
            Opcode::ToIter => {
                let a = instr.a();
                if strlist_regs.contains(&instr.b()) {
                    strlist_regs.insert(a);
                } else {
                    strlist_regs.remove(&a);
                }
                regs.remove(&a);
                str_regs.remove(&a);
            }
            // A list-shape-preserving method on a string list keeps the mark
            // (`words.map(|w| w.lower())` — a lambda returning non-strings
            // only mis-guesses toward a fallback, never a miscompile).
            Opcode::CallMethodK => {
                let a = instr.a();
                let keeps = strlist_regs.contains(&a)
                    && func
                        .consts
                        .strings
                        .get(instr.b() as usize)
                        .and_then(|name| method_role(name))
                        .is_some_and(|role| role.strlist);
                if !keeps {
                    strlist_regs.remove(&a);
                }
                regs.remove(&a);
                str_regs.remove(&a);
            }
            Opcode::LoadHeapConst => {
                let all_str = matches!(
                    func.consts.heap_values.get(instr.bx() as usize),
                    Some(ConstHeapValueData::List(elems))
                        if !elems.is_empty()
                            && elems.iter().all(|e| match e {
                                ConstRuntimeValueData::ShortStr(_) => true,
                                ConstRuntimeValueData::Heap(boxed) => {
                                    matches!(**boxed, ConstHeapValueData::LongString(_))
                                }
                                _ => false,
                            })
                );
                if all_str {
                    strlist_regs.insert(instr.a());
                } else {
                    strlist_regs.remove(&instr.a());
                }
                regs.remove(&instr.a());
                str_regs.remove(&instr.a());
            }
            Opcode::LoadString | Opcode::ConcatString | Opcode::ConcatN | Opcode::ToString => {
                str_regs.insert(instr.a());
                regs.remove(&instr.a());
                strlist_regs.remove(&instr.a());
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
            Opcode::SetFieldK if after && regs.contains(&instr.a()) => return false,
            Opcode::GetFieldK if after && regs.contains(&instr.b()) => return false,
            // Composite string-int keys are string-keyed by construction.
            Opcode::SetIndexStrI if after && regs.contains(&instr.a()) => return false,
            Opcode::GetIndexStrI if after && regs.contains(&instr.b()) => return false,
            Opcode::SetIndex if after && regs.contains(&instr.a()) => return !str_regs.contains(&instr.b()),
            Opcode::GetIndex | Opcode::GetList if after && regs.contains(&instr.b()) => {
                return !str_regs.contains(&instr.c());
            }
            Opcode::GetIndex | Opcode::GetList if strlist_regs.contains(&instr.b()) => {
                str_regs.insert(instr.a());
            }
            _ => {}
        }
    }
    false
}

/// Whether an empty `[]` literal's first pushed element is a string (tracked
/// through `Move`s, like the map-key lookahead). A wrong guess only costs a
/// fallback: the typed push rejects the mismatch, never miscompiles.
/// The first pushed value's provenance types an empty `[]` literal:
/// a string source → `ListStr`, an indexed read (`xs[i]`, a field, an
/// iterated element — possibly a boxed Dyn) → `ListDyn` (safe: everything
/// boxes), anything else → the `ListI64` default. A wrong guess only costs
/// a fallback.
#[derive(PartialEq)]
pub(crate) enum EmptyListGuess {
    Str,
    Dyn,
    Default,
}

pub(crate) fn empty_list_elem_guess(func: &FunctionData, start_pc: usize, dst_reg: u8) -> EmptyListGuess {
    let code = &func.code;
    let mut regs = std::collections::HashSet::new();
    regs.insert(dst_reg);
    let mut str_regs = std::collections::HashSet::new();
    let mut indexed_regs = std::collections::HashSet::new();
    for raw in &code[start_pc + 1..] {
        let Ok(instr) = Instr::try_from_raw(*raw) else { break };
        match instr.opcode() {
            // The literal escaping into a capture cell means closures push
            // into it — their element types are invisible here, so the
            // boxed-list guess is the only safe one (boxing never breaks
            // correctness, a typed guess would ping-pong across functions).
            Opcode::StoreCellVal if regs.contains(&instr.b()) => {
                return EmptyListGuess::Dyn;
            }
            Opcode::Move if regs.contains(&instr.b()) => {
                regs.insert(instr.a());
            }
            // A move propagates the source's provenance to the alias.
            Opcode::Move => {
                if str_regs.contains(&instr.b()) {
                    str_regs.insert(instr.a());
                    indexed_regs.remove(&instr.a());
                } else if indexed_regs.contains(&instr.b()) {
                    indexed_regs.insert(instr.a());
                    str_regs.remove(&instr.a());
                } else {
                    str_regs.remove(&instr.a());
                    indexed_regs.remove(&instr.a());
                }
            }
            Opcode::LoadString | Opcode::ConcatString | Opcode::ConcatN | Opcode::ToString => {
                str_regs.insert(instr.a());
                indexed_regs.remove(&instr.a());
            }
            // Indexed/iterated/constructed-container reads may hold a boxed
            // Dyn or a nested list — the Dyn guess is the safe one.
            Opcode::GetIndex
            | Opcode::GetList
            | Opcode::GetFieldK
            | Opcode::SliceFrom
            | Opcode::ToIter
            | Opcode::NewList
            | Opcode::NewObject => {
                indexed_regs.insert(instr.a());
                str_regs.remove(&instr.a());
            }
            // A heap constant splits by kind: a long string is still a
            // string (a `ListDyn` guess would change its display quoting);
            // lists/maps take the Dyn guess.
            Opcode::LoadHeapConst => match func.consts.heap_values.get(instr.bx() as usize) {
                Some(ConstHeapValueData::List(_) | ConstHeapValueData::Map(_)) => {
                    indexed_regs.insert(instr.a());
                    str_regs.remove(&instr.a());
                }
                _ => {
                    str_regs.insert(instr.a());
                    indexed_regs.remove(&instr.a());
                }
            },
            Opcode::AddInt => {
                if str_regs.contains(&instr.b()) || str_regs.contains(&instr.c()) {
                    str_regs.insert(instr.a());
                } else {
                    str_regs.remove(&instr.a());
                }
                indexed_regs.remove(&instr.a());
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
                indexed_regs.remove(&instr.a());
            }
            Opcode::ListPush if regs.contains(&instr.a()) => {
                return if str_regs.contains(&instr.b()) {
                    EmptyListGuess::Str
                } else if indexed_regs.contains(&instr.b()) {
                    EmptyListGuess::Dyn
                } else {
                    EmptyListGuess::Default
                };
            }
            _ => {}
        }
    }
    EmptyListGuess::Default
}

/// Interns a string constant into the module globals, returning its [`GlobalId`]
/// index (deduplicating identical strings so repeated keys share one global).
pub(crate) fn intern_global(globals: &mut Vec<String>, s: &str) -> u32 {
    if let Some(i) = globals.iter().position(|g| g == s) {
        i as u32
    } else {
        globals.push(s.to_string());
        (globals.len() - 1) as u32
    }
}
