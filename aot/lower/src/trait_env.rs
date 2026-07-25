use super::*;

/// Trait/impl information the lowering needs, taken from the module's
/// [`lk_core::vm::TypeInfo`] — the compiler's own record of what it compiled.
///
/// This used to be recovered by abstractly interpreting the entry function's
/// bytecode (tracking string/function/list values through the registration
/// call sequence) because the structured form was thrown away at compile time.
/// Now only the *positions* of those calls are scanned, so the lowering can
/// skip instructions the native path does not execute; every semantic fact
/// below comes from the artifact.
#[derive(Debug, Clone, Default)]
pub(crate) struct TraitEnv {
    /// `(type name, method name)` → impl function index.
    pub(crate) impls: std::collections::HashMap<(String, String), u32>,
    /// Type name → runtime type id (1-based, declaration order).
    pub(crate) type_ids: std::collections::HashMap<String, i64>,
    /// Method name → dispatch arms `(type id, impl fn)`, declaration order.
    pub(crate) methods: std::collections::HashMap<String, Vec<(i64, u32)>>,
    /// Entry pcs covered by registration sequences (skipped when lowering).
    pub(crate) skip_pcs: std::collections::HashSet<usize>,
}

pub(crate) fn trait_env_prescan(module: &lk_core::vm::ModuleData) -> TraitEnv {
    let mut env = TraitEnv::default();

    // Semantics: straight from the compiler's declarations. Declaration order
    // fixes the runtime type ids, so the ordering here is load-bearing.
    for decl in &module.type_info.impls {
        let next_id = env.type_ids.len() as i64 + 1;
        let tid = *env.type_ids.entry(decl.type_name.clone()).or_insert(next_id);
        for method in &decl.methods {
            env.impls
                .insert((decl.type_name.clone(), method.name.clone()), method.function);
            let arms = env.methods.entry(method.name.clone()).or_default();
            arms.retain(|&(t, _)| t != tid);
            arms.push((tid, method.function));
        }
    }

    env.skip_pcs = registration_pcs(module);
    env
}

/// Positions of the compiler's `__lk_register_trait{,_impl}` call sequences in
/// the entry function.
///
/// These instructions build and perform a *runtime* registration that the
/// native path replaces with [`TraitEnv`], so the lowering skips them. Only
/// the extent of each sequence matters here — what it registers is already
/// known — so this walks the same contiguous straight-line shape the compiler
/// emits (helper `GetGlobal`, then only value/list building, then the `Call`)
/// and records its pcs.
fn registration_pcs(module: &lk_core::vm::ModuleData) -> std::collections::HashSet<usize> {
    let mut skip = std::collections::HashSet::new();
    let Some(entry_fn) = module.functions.get(module.entry as usize) else {
        return skip;
    };
    let mut seq_pcs: Vec<usize> = Vec::new();
    // Registers currently holding the helper callable. A set rather than a
    // single register because the compiler may `Move` it into the call window.
    let mut helper_regs: std::collections::HashSet<u8> = std::collections::HashSet::new();
    for (pc, raw) in entry_fn.code.iter().enumerate() {
        let Ok(instr) = Instr::try_from_raw(*raw) else {
            helper_regs.clear();
            seq_pcs.clear();
            continue;
        };
        match instr.opcode() {
            Opcode::GetGlobal => {
                let is_helper = matches!(
                    module.globals.get(instr.bx() as usize).map(String::as_str),
                    Some("__lk_register_trait") | Some("__lk_register_trait_impl")
                );
                if is_helper {
                    if helper_regs.is_empty() {
                        seq_pcs.clear();
                    }
                    helper_regs.insert(instr.a());
                    seq_pcs.push(pc);
                } else {
                    helper_regs.remove(&instr.a());
                    if helper_regs.is_empty() {
                        seq_pcs.clear();
                    }
                }
            }
            Opcode::Move if !helper_regs.is_empty() => {
                if helper_regs.contains(&instr.b()) {
                    helper_regs.insert(instr.a());
                } else {
                    helper_regs.remove(&instr.a());
                }
                seq_pcs.push(pc);
            }
            // The value/list building the call window needs; each overwrites
            // its destination, so a helper register it clobbers is dropped.
            Opcode::LoadString | Opcode::LoadHeapConst | Opcode::LoadFunction | Opcode::NewList
                if !helper_regs.is_empty() =>
            {
                helper_regs.remove(&instr.a());
                seq_pcs.push(pc);
            }
            Opcode::Call if helper_regs.contains(&instr.a()) => {
                seq_pcs.push(pc);
                skip.extend(seq_pcs.drain(..));
                helper_regs.clear();
            }
            _ => {
                // Anything else ends the candidate: the sequence is only
                // trusted while it stays contiguous straight-line output.
                helper_regs.clear();
                seq_pcs.clear();
            }
        }
    }
    skip
}
