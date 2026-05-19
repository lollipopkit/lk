use anyhow::anyhow;
use arcstr::ArcStr;

#[cfg(not(feature = "aot-minimal-runtime"))]
use crate::val::NativeArgs;
use crate::val::{Val, methods::find_method_for_val};

use super::VmContext;

#[cfg(not(feature = "aot-minimal-runtime"))]
pub(super) fn core_call_method_builtin_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> anyhow::Result<Val> {
    core_call_method_builtin(args.as_slice(), ctx)
}

pub(super) fn method_name_arc(helper: &str, method: &Val) -> anyhow::Result<ArcStr> {
    match method {
        Val::Str(s) => Ok(s.clone()),
        Val::ShortStr(s) => Ok(Val::intern_str(s.as_str())),
        other => Err(anyhow!(
            "{} expects method name as string, got {}",
            helper,
            other.type_name()
        )),
    }
}

pub(super) fn call_method_positional(
    receiver: Val,
    method_arc: ArcStr,
    positional_args: &[Val],
    ctx: &mut VmContext,
) -> anyhow::Result<Val> {
    let method_key = Val::Str(method_arc.clone());
    if positional_args.is_empty()
        && let Some(prop_val) = receiver.access(&method_key)
    {
        match prop_val {
            Val::Closure(_)
            | Val::RustFunction(_)
            | Val::RustFastFunction(_)
            | Val::RustFastFunctionNamed(_)
            | Val::RustFunctionNamed(_) => {
                return prop_val.call(&[], ctx);
            }
            other => return Ok(other),
        }
    }

    if let Some(prop_val) = receiver.access(&method_key) {
        match prop_val {
            Val::Closure(_)
            | Val::RustFunction(_)
            | Val::RustFastFunction(_)
            | Val::RustFastFunctionNamed(_)
            | Val::RustFunctionNamed(_) => {
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

    if let Some(func) = find_method_for_val(&receiver, method_arc.as_str()) {
        let mut full_args = Vec::with_capacity(positional_args.len() + 1);
        full_args.push(receiver.clone());
        full_args.extend(positional_args.iter().cloned());
        return func.call(&full_args, ctx);
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
        Val::List(list) => list.iter().cloned().collect(),
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
pub(super) fn core_call_method_named_builtin_fast(args: NativeArgs<'_>, ctx: &mut VmContext) -> anyhow::Result<Val> {
    core_call_method_named_builtin(args.as_slice(), ctx)
}

fn core_call_method_named_builtin(args: &[Val], ctx: &mut VmContext) -> anyhow::Result<Val> {
    if args.len() != 4 {
        return Err(anyhow!(
            "__lk_call_method_named expects 4 arguments: receiver, method name, positional args list, named args map"
        ));
    }

    let receiver = args[0].clone();
    let method_arc: ArcStr = match &args[1] {
        Val::Str(s) => s.clone(),
        Val::ShortStr(s) => Val::intern_str(s.as_str()),
        other => {
            return Err(anyhow!(
                "__lk_call_method_named expects method name as string, got {}",
                other.type_name()
            ));
        }
    };
    let positional_args: Vec<Val> = match &args[2] {
        Val::List(list) => list.iter().cloned().collect(),
        Val::Nil => Vec::new(),
        other => {
            return Err(anyhow!(
                "__lk_call_method_named expects positional arguments as list, got {}",
                other.type_name()
            ));
        }
    };
    let named_pairs: Vec<(String, Val)> = match &args[3] {
        Val::Map(map) => map.iter().map(|(k, v)| (k.to_string(), v.clone())).collect(),
        Val::Nil => Vec::new(),
        other => {
            return Err(anyhow!(
                "__lk_call_method_named expects named arguments as map, got {}",
                other.type_name()
            ));
        }
    };
    let method_key = Val::Str(method_arc.clone());

    if let Some(prop_val) = receiver.access(&method_key) {
        match prop_val {
            Val::Closure(_) | Val::RustFastFunctionNamed(_) | Val::RustFunctionNamed(_) => {
                return prop_val.call_named(&positional_args, &named_pairs, ctx);
            }
            Val::RustFunction(_) | Val::RustFastFunction(_) => {
                if named_pairs.is_empty() {
                    return prop_val.call(&positional_args, ctx);
                }
                return Err(anyhow!("Named arguments are not supported for native functions"));
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

    if let Some(func) = find_method_for_val(&receiver, method_arc.as_str()) {
        if !named_pairs.is_empty() {
            return Err(anyhow!("Named arguments are not supported for built-in methods"));
        }
        let mut full_args = Vec::with_capacity(positional_args.len() + 1);
        full_args.push(receiver.clone());
        full_args.extend(positional_args);
        return func.call(&full_args, ctx);
    }

    Err(anyhow!("{} has no method '{}'", receiver.type_name(), method_arc))
}
