use crate::llvm::straightline_value::{
    NativeBuiltin, NativeStraightlineValue, native_runtime_const_value, native_static_index, native_static_len,
};

use super::list_methods::emit_native_static_list_method;

pub(super) fn emit_native_iter_builtin(
    builtin: NativeBuiltin,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match builtin {
        NativeBuiltin::IterEnumerate => emit_native_iter_list_method("enumerate", args, ssa_index),
        NativeBuiltin::IterTake => emit_native_iter_list_method_with_arg("take", args, ssa_index),
        NativeBuiltin::IterSkip => emit_native_iter_list_method_with_arg("skip", args, ssa_index),
        NativeBuiltin::IterChain => emit_native_iter_list_method_with_arg("chain", args, ssa_index),
        NativeBuiltin::IterFlatten => emit_native_iter_list_method("flatten", args, ssa_index),
        NativeBuiltin::IterUnique => emit_native_iter_list_method("unique", args, ssa_index),
        NativeBuiltin::IterChunk => emit_native_iter_list_method_with_arg("chunk", args, ssa_index),
        NativeBuiltin::IterZip => emit_native_iter_list_method_with_arg("zip", args, ssa_index),
        _ => None,
    }
}

pub(super) fn emit_native_iter_module_method(
    method: &str,
    args: &[NativeStraightlineValue],
    _ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    match method {
        "next" => emit_native_iter_next(args),
        "collect" => emit_native_iter_collect(args),
        _ => None,
    }
}

fn emit_native_iter_next(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    native_static_index(
        target.clone(),
        NativeStraightlineValue::I64("0".to_string()),
        String::new(),
    )
}

fn emit_native_iter_collect(args: &[NativeStraightlineValue]) -> Option<NativeStraightlineValue> {
    let [target @ NativeStraightlineValue::List { .. }] = args else {
        return None;
    };
    native_static_len(target.clone())?;
    Some(target.clone())
}

fn emit_native_iter_list_method(
    method: &str,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [target] = args else {
        return None;
    };
    emit_native_static_list_method(target.clone(), method, &[], ssa_index)
}

fn emit_native_iter_list_method_with_arg(
    method: &str,
    args: &[NativeStraightlineValue],
    ssa_index: &mut usize,
) -> Option<NativeStraightlineValue> {
    let [target, arg] = args else {
        return None;
    };
    emit_native_static_list_method(target.clone(), method, &[native_runtime_const_value(arg)?], ssa_index)
}
