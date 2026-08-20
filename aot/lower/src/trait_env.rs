/// Trait/impl information the lowering needs, taken from the module's
/// [`lk_core::vm::TypeInfo`] — the compiler's own record of what it compiled.
///
/// This used to be recovered by abstractly interpreting the entry function's
/// bytecode — the compiler serialized `impl` blocks into string literals plus
/// a runtime registration call, so every consumer had to decode them again.
/// The declarations now travel structurally in the artifact and the
/// registration calls are gone, so this is a direct read.
/// One declared struct as the entry prologue describes it to the runtime: its
/// type id, its name, and `(field name, declared-type code)` in declaration
/// order.
pub(crate) type StructTypeDecl = (i64, String, Vec<(String, i64)>);

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
    pub(crate) struct_fields: Vec<StructTypeDecl>,
    /// `(struct name, field name)` → the field's position in the declaration.
    ///
    /// A declared struct's fields are a fixed, ordered list, so a field read
    /// can be a positional one rather than a hash lookup (`map_h.str_dyn_get_at`).
    pub(crate) struct_field_index: std::collections::HashMap<(String, String), usize>,
    /// `(struct name, field name)` → the declared-type code a store is measured
    /// against (`DECLARED_*`).
    pub(crate) struct_field_codes: std::collections::HashMap<(String, String), i64>,
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

/// The dispatch id of a built-in impl target, or `None` for a struct name.
///
/// Mirrors `lkrt::lkdyn::dispatch_builtin_code` and its base — the runtime
/// computes the same number from the value's tag, and the two have to agree or
/// no arm matches. `examples/syntax/trait_builtin.lk` is the conformance check:
/// it dispatches through a trait parameter on every built-in kind, so a
/// disagreement is a wrong answer there rather than a silent miss.
///
/// The name is the impl target's type *text*, so a container's is written out
/// (`List<Any>`, `Map<Any, Any>`) and only its base names the type.
fn dispatch_builtin_type_id(type_name: &str) -> Option<i64> {
    const BASE: i64 = 1 << 40;
    let code = match type_name.split('<').next().unwrap_or(type_name) {
        "Nil" => 1,
        "Bool" => 2,
        "Int" => 3,
        "Float" => 4,
        "String" => 5,
        "List" => 6,
        "Set" => 7,
        "Bytes" => 8,
        "Map" => 9,
        _ => return None,
    };
    Some(BASE + code)
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
            decl.fields
                .iter()
                .map(|f| (f.name.clone(), declared_field_code(f.ty.as_deref())))
                .collect(),
        ));
        for (index, field) in decl.fields.iter().enumerate() {
            env.struct_field_index
                .insert((decl.name.clone(), field.name.clone()), index);
            env.struct_field_codes.insert(
                (decl.name.clone(), field.name.clone()),
                declared_field_code(field.ty.as_deref()),
            );
        }
    }
    for decl in &module.type_info.impls {
        let next_id = env.type_ids.len() as i64 + 1;
        // `impl S for Int` names a *built-in* type, whose values carry no arena
        // mark for a sequential id to be compared against. Those arms take the
        // fixed code the runtime answers for the kind
        // (`lkrt::lkdyn::dispatch_builtin_code`); a struct keeps the sequential
        // id, which is also what `display` looks its name up by.
        let tid = match dispatch_builtin_type_id(&decl.type_name) {
            Some(fixed) => *env.type_ids.entry(decl.type_name.clone()).or_insert(fixed),
            None => *env.type_ids.entry(decl.type_name.clone()).or_insert(next_id),
        };
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

/// The declared-type code `obj_ty.field` carries to the runtime, mirroring
/// `lkrt::lkdyn`'s constants. Scalars only, and `Any` for everything else — see
/// `lkrt::lkdyn::check_declared_field`.
pub(crate) const DECLARED_ANY: i64 = 0;
pub(crate) const DECLARED_INT: i64 = 1;
pub(crate) const DECLARED_FLOAT: i64 = 2;
pub(crate) const DECLARED_BOOL: i64 = 3;
pub(crate) const DECLARED_STR: i64 = 4;
pub(crate) const DECLARED_NULLABLE: i64 = 16;

fn declared_field_code(text: Option<&str>) -> i64 {
    use lk_core::val::Type;
    let Some(ty) = text.and_then(Type::parse) else {
        return DECLARED_ANY;
    };
    let (ty, nullable) = match &ty {
        Type::Optional(inner) => ((**inner).clone(), DECLARED_NULLABLE),
        other => (other.clone(), 0),
    };
    let base = match ty {
        Type::Int => DECLARED_INT,
        Type::Float => DECLARED_FLOAT,
        Type::Bool => DECLARED_BOOL,
        Type::String => DECLARED_STR,
        _ => return DECLARED_ANY,
    };
    base | nullable
}
