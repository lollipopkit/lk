/// Trait/impl information the lowering needs, taken from the module's
/// [`lk_core::vm::TypeInfo`] — the compiler's own record of what it compiled.
///
/// This used to be recovered by abstractly interpreting the entry function's
/// bytecode — the compiler serialized `impl` blocks into string literals plus
/// a runtime registration call, so every consumer had to decode them again.
/// The declarations now travel structurally in the artifact and the
/// registration calls are gone, so this is a direct read.
#[derive(Debug, Clone, Default)]
pub(crate) struct TraitEnv {
    /// `(type name, method name)` → impl function index.
    pub(crate) impls: std::collections::HashMap<(String, String), u32>,
    /// Type name → runtime type id (1-based, declaration order).
    pub(crate) type_ids: std::collections::HashMap<String, i64>,
    /// Method name → dispatch arms `(type id, impl fn)`, declaration order.
    pub(crate) methods: std::collections::HashMap<String, Vec<(i64, u32)>>,
}

pub(crate) fn trait_env_prescan(module: &lk_core::vm::ModuleData) -> TraitEnv {
    let mut env = TraitEnv::default();
    // Declaration order fixes the runtime type ids, so the ordering here is
    // load-bearing.
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
    env
}
