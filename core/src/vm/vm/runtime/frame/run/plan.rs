use std::sync::Arc;

use anyhow::{Result, anyhow};

use crate::val::{ClosureValue, Type, Val};
use crate::vm::vm::caches::NamedCallPlan;

pub(super) fn build_named_call_plan(closure: &ClosureValue, named_slice: &[Val]) -> Result<Arc<NamedCallPlan>> {
    if named_slice.len() % 2 != 0 {
        return Err(anyhow!(
            "Named argument slice has unexpected odd length: {}",
            named_slice.len()
        ));
    }
    let named_params = closure.named_params.as_ref();
    let total_named = named_params.len();
    let mut provided_flags = vec![false; total_named];
    let mut provided_indices = Vec::with_capacity(named_slice.len() / 2);
    let index_by_name = closure.named_param_index();
    for pair in named_slice.chunks_exact(2) {
        let key_val = &pair[0];
        let key_str = match key_val {
            Val::Str(s) => s.as_ref(),
            Val::Int(v) => return Err(anyhow!("Named argument key must be a string, got Int({})", v)),
            Val::Float(v) => return Err(anyhow!("Named argument key must be a string, got Float({})", v)),
            Val::Bool(v) => return Err(anyhow!("Named argument key must be a string, got Bool({})", v)),
            other => return Err(anyhow!("Named argument key must be primitive, got {:?}", other)),
        };
        let idx = if let Some(idx) = index_by_name.get(key_str) {
            *idx
        } else {
            return Err(anyhow!("Unknown named argument: {}", key_str));
        };
        if provided_flags[idx] {
            return Err(anyhow!("Duplicate named argument: {}", key_str));
        }
        provided_flags[idx] = true;
        provided_indices.push(idx);
    }

    let mut defaults_to_eval = Vec::new();
    let mut optional_nil = Vec::new();
    for idx in 0..total_named {
        if provided_flags[idx] {
            continue;
        }
        if closure.default_funcs.get(idx).and_then(|opt| opt.as_ref()).is_some() {
            defaults_to_eval.push(idx);
        } else if matches!(named_params[idx].type_annotation, Some(Type::Optional(_))) {
            optional_nil.push(idx);
        } else {
            return Err(anyhow!("Missing required named argument: {}", named_params[idx].name));
        }
    }

    Ok(Arc::new(NamedCallPlan {
        provided_indices: Arc::from(provided_indices.into_boxed_slice()),
        defaults_to_eval: Arc::from(defaults_to_eval.into_boxed_slice()),
        optional_nil: Arc::from(optional_nil.into_boxed_slice()),
    }))
}
