use anyhow::anyhow;
use arcstr::ArcStr;

use crate::val::{RuntimeVal, Val, runtime_val_to_val, val_to_runtime_val};
use crate::vm::{NativeArgs32, NativeRuntime32};

use super::VmContext;

pub(super) fn method_name_arc(helper: &str, method: &Val) -> anyhow::Result<ArcStr> {
    if let Some(method) = method.as_str() {
        Ok(Val::intern_str(method))
    } else {
        Err(anyhow!(
            "{} expects method name as string, got {}",
            helper,
            method.type_name()
        ))
    }
}

pub(super) fn call_method_positional(
    receiver: Val,
    method_arc: ArcStr,
    positional_args: &[Val],
    ctx: &mut VmContext,
) -> anyhow::Result<Val> {
    let method_key = Val::from_str(method_arc.as_str());
    if positional_args.is_empty()
        && let Some(prop_val) = receiver.access(&method_key)
    {
        match prop_val {
            ref callable if callable.is_callable() => {
                return prop_val.call(&[], ctx);
            }
            other => return Ok(other),
        }
    }

    if let Some(prop_val) = receiver.access(&method_key) {
        match prop_val {
            ref callable if callable.is_callable() => {
                return prop_val.call(positional_args, ctx);
            }
            other => {
                if positional_args.is_empty() {
                    return Ok(other);
                }
            }
        }
    }

    if let Some(tc) = ctx.type_checker().as_ref() {
        let obj_type = receiver.dispatch_type();
        if let Some(method_val) = tc.registry().get_method(&obj_type, method_arc.as_str()) {
            let mut full_args = Vec::with_capacity(positional_args.len() + 1);
            full_args.push(receiver.clone());
            full_args.extend(positional_args.iter().cloned());
            return method_val.clone().call(&full_args, ctx);
        }
    }

    Err(anyhow!("{} has no method '{}'", receiver.type_name(), method_arc))
}

fn core_call_method_builtin(args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 3 {
        return Err(anyhow!(
            "__lk_call_method expects 3 arguments: receiver, method name, positional args list"
        ));
    }

    let receiver = args[0].clone();
    let method_arc = method_name_arc("__lk_call_method", &args[1])?;

    let positional_args: Vec<Val> = match &args[2] {
        value if value.as_list().is_some() => value.as_list().expect("checked list").iter().cloned().collect(),
        Val::Nil => Vec::new(),
        other => {
            return Err(anyhow!(
                "__lk_call_method expects positional arguments as list, got {}",
                other.type_name()
            ));
        }
    };
    call_method_positional(receiver, method_arc, &positional_args, ctx)
}

#[cfg(not(feature = "aot-minimal-runtime"))]
pub(super) fn core_call_method_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    let values = runtime_args_to_vals(args.as_slice(), runtime)?;
    let ctx = runtime
        .ctx
        .as_deref_mut()
        .ok_or_else(|| anyhow!("__lk_call_method requires VmContext"))?;
    let value = core_call_method_builtin(&values, ctx)?;
    val_to_runtime_val(&value, runtime.heap_mut())
}

fn core_call_method_named_builtin(args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 4 {
        return Err(anyhow!(
            "__lk_call_method_named expects 4 arguments: receiver, method name, positional args list, named args map"
        ));
    }

    let receiver = args[0].clone();
    let method_arc = method_name_arc("__lk_call_method_named", &args[1])?;
    let positional_args: Vec<Val> = match &args[2] {
        value if value.as_list().is_some() => value.as_list().expect("checked list").iter().cloned().collect(),
        Val::Nil => Vec::new(),
        other => {
            return Err(anyhow!(
                "__lk_call_method_named expects positional arguments as list, got {}",
                other.type_name()
            ));
        }
    };
    let named_pairs: Vec<(String, Val)> = match &args[3] {
        value if value.as_map().is_some() => value
            .as_map()
            .expect("checked map")
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
        Val::Nil => Vec::new(),
        other => {
            return Err(anyhow!(
                "__lk_call_method_named expects named arguments as map, got {}",
                other.type_name()
            ));
        }
    };
    let method_key = Val::from_str(method_arc.as_str());

    if let Some(prop_val) = receiver.access(&method_key) {
        match prop_val {
            prop_val if prop_val.is_callable() => {
                return prop_val.call_named(&positional_args, &named_pairs, ctx);
            }
            other => {
                if positional_args.is_empty() && named_pairs.is_empty() {
                    return Ok(other);
                }
            }
        }
    }

    if !named_pairs.is_empty()
        && let Some(tc) = ctx.type_checker().as_ref()
        && tc
            .registry()
            .get_method(&receiver.dispatch_type(), method_arc.as_str())
            .is_some()
    {
        return Err(anyhow!("Named arguments are not supported for trait methods"));
    }

    if let Some(tc) = ctx.type_checker().as_ref() {
        let obj_type = receiver.dispatch_type();
        if let Some(method_val) = tc.registry().get_method(&obj_type, method_arc.as_str()) {
            let mut full_args = Vec::with_capacity(positional_args.len() + 1);
            full_args.push(receiver.clone());
            full_args.extend(positional_args.iter().cloned());
            return method_val.clone().call(&full_args, ctx);
        }
    }

    Err(anyhow!("{} has no method '{}'", receiver.type_name(), method_arc))
}

#[cfg(not(feature = "aot-minimal-runtime"))]
pub(super) fn core_call_method_named_builtin32(
    args: NativeArgs32<'_>,
    runtime: &mut NativeRuntime32<'_>,
) -> anyhow::Result<RuntimeVal> {
    let values = runtime_args_to_vals(args.as_slice(), runtime)?;
    let ctx = runtime
        .ctx
        .as_deref_mut()
        .ok_or_else(|| anyhow!("__lk_call_method_named requires VmContext"))?;
    let value = core_call_method_named_builtin(&values, ctx)?;
    val_to_runtime_val(&value, runtime.heap_mut())
}

#[cfg(not(feature = "aot-minimal-runtime"))]
fn runtime_args_to_vals(args: &[RuntimeVal], runtime: &NativeRuntime32<'_>) -> anyhow::Result<Vec<Val>> {
    args.iter()
        .map(|value| runtime_val_to_val(value, &runtime.state.heap))
        .collect()
}
