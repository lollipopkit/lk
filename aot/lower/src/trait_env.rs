use super::*;

/// Trait/impl registrations extracted from the entry's compiler-generated
/// `__lk_register_trait` / `__lk_register_trait_impl` call sequences (plan
/// J1). The registration itself never runs natively: the prescan lifts each
/// sequence into this table and marks its pcs skipped; method calls
/// devirtualize statically through `impls` (known `NewObject` provenance) or
/// dynamically through `methods` (boxed receivers, via arena type marks).
#[derive(Debug, Clone, Default)]
pub(crate) struct TraitEnv {
    /// `(type name, method name)` → impl function index.
    pub(crate) impls: std::collections::HashMap<(String, String), u32>,
    /// Type name → runtime type id (1-based, first-registration order).
    pub(crate) type_ids: std::collections::HashMap<String, i64>,
    /// Method name → dispatch arms `(type id, impl fn)`, registration order.
    pub(crate) methods: std::collections::HashMap<String, Vec<(i64, u32)>>,
    /// Entry pcs covered by registration sequences (skipped when lowering).
    pub(crate) skip_pcs: std::collections::HashSet<usize>,
}

/// The abstract register contents the registration prescan tracks — exactly
/// the value kinds the compiler's `lower_trait_decl`/`lower_impl_decl` emit.
#[derive(Debug, Clone)]
pub(crate) enum TraitAbs {
    Str(String),
    Fn(u32),
    List(Vec<TraitAbs>),
    /// The helper callable; `true` = `__lk_register_trait_impl`.
    Helper(bool),
}

pub(crate) fn trait_env_prescan(module: &lk_core::vm::ModuleData) -> TraitEnv {
    let mut env = TraitEnv::default();
    let Some(entry_fn) = module.functions.get(module.entry as usize) else {
        return env;
    };
    // A registration sequence is contiguous straight-line compiler output:
    // the helper `GetGlobal`, then only Load*/Move/NewList building the call
    // window, then the `Call`. Anything unexpected abandons the candidate —
    // its `GetGlobal` then rejects loudly during lowering (never a silent
    // half-registration).
    let mut regs: std::collections::HashMap<u8, TraitAbs> = std::collections::HashMap::new();
    let mut seq_pcs: Vec<usize> = Vec::new();
    let mut in_seq = false;
    for (pc, raw) in entry_fn.code.iter().enumerate() {
        let Ok(instr) = Instr::try_from_raw(*raw) else {
            regs.clear();
            in_seq = false;
            continue;
        };
        match instr.opcode() {
            Opcode::GetGlobal => {
                let is_impl = match module.globals.get(instr.bx() as usize).map(String::as_str) {
                    Some("__lk_register_trait") => Some(false),
                    Some("__lk_register_trait_impl") => Some(true),
                    _ => None,
                };
                let Some(is_impl) = is_impl else {
                    regs.remove(&instr.a());
                    in_seq = false;
                    continue;
                };
                regs.insert(instr.a(), TraitAbs::Helper(is_impl));
                seq_pcs = vec![pc];
                in_seq = true;
            }
            Opcode::LoadString if in_seq => {
                let text = entry_fn.consts.strings.get(instr.bx() as usize).cloned();
                match text {
                    Some(text) => {
                        regs.insert(instr.a(), TraitAbs::Str(text));
                        seq_pcs.push(pc);
                    }
                    None => in_seq = false,
                }
            }
            Opcode::LoadHeapConst if in_seq => {
                // Method-type display strings land in the heap-const pool
                // when longer than the inline form.
                match entry_fn.consts.heap_values.get(instr.bx() as usize) {
                    Some(ConstHeapValueData::LongString(text)) => {
                        regs.insert(instr.a(), TraitAbs::Str(text.to_string()));
                        seq_pcs.push(pc);
                    }
                    _ => in_seq = false,
                }
            }
            Opcode::LoadFunction if in_seq => {
                regs.insert(instr.a(), TraitAbs::Fn(instr.bx() as u32));
                seq_pcs.push(pc);
            }
            Opcode::Move if in_seq => match regs.get(&instr.b()).cloned() {
                Some(v) => {
                    regs.insert(instr.a(), v);
                    seq_pcs.push(pc);
                }
                None => in_seq = false,
            },
            Opcode::NewList if in_seq => {
                let mut items = Vec::with_capacity(instr.c() as usize);
                for i in 0..instr.c() {
                    match regs.get(&instr.b().wrapping_add(i)) {
                        Some(v) => items.push(v.clone()),
                        None => {
                            in_seq = false;
                            break;
                        }
                    }
                }
                if in_seq {
                    regs.insert(instr.a(), TraitAbs::List(items));
                    seq_pcs.push(pc);
                }
            }
            Opcode::Call if in_seq => {
                in_seq = false;
                let base = instr.a();
                let argc = instr.c() as usize;
                let arg = |i: u8| regs.get(&base.wrapping_add(1).wrapping_add(i));
                match regs.get(&base) {
                    Some(TraitAbs::Helper(false)) if argc == 2 => {
                        // Trait declaration: nothing to record, skip the pcs.
                        seq_pcs.push(pc);
                        env.skip_pcs.extend(seq_pcs.drain(..));
                    }
                    Some(TraitAbs::Helper(true)) if argc == 3 => {
                        let (
                            Some(TraitAbs::Str(_trait_name)),
                            Some(TraitAbs::Str(type_name)),
                            Some(TraitAbs::List(entries)),
                        ) = (arg(0), arg(1), arg(2))
                        else {
                            continue;
                        };
                        let mut extracted = Vec::with_capacity(entries.len());
                        for entry in entries {
                            let TraitAbs::List(parts) = entry else {
                                extracted.clear();
                                break;
                            };
                            let (Some(TraitAbs::Str(method)), Some(TraitAbs::Fn(fidx))) = (parts.first(), parts.get(1))
                            else {
                                extracted.clear();
                                break;
                            };
                            extracted.push((method.clone(), *fidx));
                        }
                        if extracted.is_empty() {
                            continue;
                        }
                        let next_id = env.type_ids.len() as i64 + 1;
                        let tid = *env.type_ids.entry(type_name.clone()).or_insert(next_id);
                        for (method, fidx) in extracted {
                            env.impls.insert((type_name.clone(), method.clone()), fidx);
                            let arms = env.methods.entry(method).or_default();
                            arms.retain(|&(t, _)| t != tid);
                            arms.push((tid, fidx));
                        }
                        seq_pcs.push(pc);
                        env.skip_pcs.extend(seq_pcs.drain(..));
                    }
                    _ => {}
                }
            }
            _ => {
                // Branches, stores, anything else: the abstract state is
                // only trusted within a contiguous builder sequence.
                regs.clear();
                in_seq = false;
            }
        }
    }
    env
}
