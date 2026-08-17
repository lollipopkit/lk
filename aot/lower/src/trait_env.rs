use lk_aot_mir::Ty;

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
    /// Type name → its field names in **declaration order**, which is the order
    /// `display` prints them in (see `val::DeclaredType::fields`).
    ///
    /// Emitted into the entry prologue as `obj_ty.begin`/`obj_ty.field` calls so
    /// the runtime can render a marked instance. Ordered by type id, so the
    /// emission order is fixed.
    pub(crate) struct_fields: Vec<(i64, String, Vec<String>)>,
    /// `(struct name, field name)` → the field's **declared** type, for the
    /// fields that have one.
    ///
    /// A field read is `map_h.str_dyn_get`, whose result is a boxed `Dyn`: the
    /// storage is a string-keyed map and knows nothing about declarations. The
    /// declaration does, and it is the only thing that does — without it
    /// `p.count + 1` boxed both operands and called `dyn.add` for a field the
    /// program said was an `Int`, and a loop variable fed from one could not
    /// stay an integer at all.
    pub(crate) struct_field_tys: std::collections::HashMap<(String, String), Ty>,
}

/// Method names the lowering may call **without** a `CallMethodK` naming them.
///
/// `show` is reached from a display site (`"${value}"`), not from a method call
/// — see `lower_method::apply_show`. Anything added there has to be added here
/// too, or its impl stops being a lowering root and the module fails MIR
/// validation with a dangling callee. One list, named at both ends.
pub(crate) const IMPLICIT_METHOD_HOOKS: &[&str] = &["show"];

/// Every method name some `CallMethodK` in the module names.
///
/// `CallMethodK` is the only method-call opcode and it takes its name from the
/// constant pool, so this set is exact — there is no dynamic-name form to be
/// conservative about.
pub(crate) fn called_method_names(module: &lk_core::vm::ModuleData) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for func in &module.functions {
        for raw in &func.code {
            let Ok(instr) = lk_core::vm::Instr::try_from_raw(*raw) else {
                break;
            };
            if instr.opcode() == lk_core::vm::Opcode::CallMethodK
                && let Some(name) = func.consts.strings.get(instr.b() as usize)
            {
                names.insert(name.to_string());
            }
        }
    }
    names
}

impl TraitEnv {
    /// The type whose `impl` block defines function `fidx`, if any.
    ///
    /// The inverse of [`Self::impls`], and what tells the lowering that `self`
    /// inside an impl method is that type — provenance a parameter cannot get
    /// from a `NewObject` because it never sees one.
    ///
    /// Linear over the table: impl blocks are counted in the dozens, and this
    /// runs once per lowered function.
    pub(crate) fn impl_owner(&self, fidx: u32) -> Option<String> {
        let mut found: Option<&String> = None;
        for ((type_name, _), &f) in &self.impls {
            if f != fidx {
                continue;
            }
            match found {
                // One function registered under two types — the compiler is
                // free to share a body, and a *default* method copied into two
                // impls is exactly two identical bodies. Answering either type
                // would devirtualize `self.other()` to the wrong impl, which is
                // a wrong answer rather than a refusal. So: no answer.
                Some(previous) if previous != type_name => return None,
                Some(_) => {}
                None => found = Some(type_name),
            }
        }
        found.cloned()
    }
}

pub(crate) fn trait_env_prescan(module: &lk_core::vm::ModuleData) -> TraitEnv {
    let mut env = TraitEnv::default();
    // Declaration order fixes the runtime type ids, so the ordering here is
    // load-bearing.
    // Every declared struct gets a type id, not only the ones with impls: the
    // id is also how `display` finds a type's name and field order, and a
    // struct with no methods still prints.
    for decl in &module.type_info.structs {
        let next_id = env.type_ids.len() as i64 + 1;
        let tid = *env.type_ids.entry(decl.name.clone()).or_insert(next_id);
        env.struct_fields.push((
            tid,
            decl.name.clone(),
            decl.fields.iter().map(|f| f.name.clone()).collect(),
        ));
        for field in &decl.fields {
            let Some(text) = field.ty.as_deref() else {
                continue;
            };
            let Some(ty) = lk_core::val::Type::parse(text).as_ref().and_then(declared_field_ty) else {
                continue;
            };
            env.struct_field_tys.insert((decl.name.clone(), field.name.clone()), ty);
        }
    }
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

/// The MIR type a declared field type unboxes to, when it is one this lowering
/// can hold unboxed.
///
/// Scalars only, deliberately. A container field is a *handle* whose carrier
/// the declaration does not pin (`List<Int>` may be any list representation),
/// and unboxing one to a guessed carrier is how a wrong answer gets made; the
/// boxed read those already do is correct.
fn declared_field_ty(ty: &lk_core::val::Type) -> Option<Ty> {
    use lk_core::val::Type;
    match ty {
        Type::Int => Some(Ty::I64),
        Type::Float => Some(Ty::F64),
        Type::String => Some(Ty::Str),
        // `Bool` stays boxed. `dyn.as_bool` returns the ABI's `I64`, while MIR
        // `Ty::Bool` is a one-bit value codegen `uextend`s — claiming the type
        // without the comparison that produces it fails the Cranelift verifier.
        // Narrowing it needs that conversion, which is a separate step.
        _ => None,
    }
}
