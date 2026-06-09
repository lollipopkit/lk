use crate::llvm::stdlib_catalog::stdlib_module_index;

use super::{NativeBuiltin, NativeModule, NativeStraightlineValue};

pub(super) fn native_static_module_index(
    module: NativeModule,
    key: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    if let Some(value) = native_core_container_module_index(module, key.clone()) {
        return Some(value);
    }
    stdlib_module_index(module, key)
}

fn native_core_container_module_index(
    module: NativeModule,
    key: NativeStraightlineValue,
) -> Option<NativeStraightlineValue> {
    let NativeStraightlineValue::String { value: key, .. } = key else {
        return None;
    };
    let builtin = match (module.name(), key.as_str()) {
        ("example.fib", "iterative") => NativeBuiltin::FibIterative,
        ("example.mathlib", "double") => NativeBuiltin::MathlibDouble,
        ("example.greetings", "message") => NativeBuiltin::GreetingsMessage,
        ("map", "len") => NativeBuiltin::MapModuleMethod("len"),
        ("map", "keys") => NativeBuiltin::MapModuleMethod("keys"),
        ("map", "values") => NativeBuiltin::MapModuleMethod("values"),
        ("map", "has") => NativeBuiltin::MapModuleMethod("has"),
        ("map", "get") => NativeBuiltin::MapModuleMethod("get"),
        ("map", "delete") => NativeBuiltin::MapDelete,
        ("map", "set") => NativeBuiltin::MapSet,
        ("map", "mutate") => NativeBuiltin::MapMutate,
        ("list", "concat") => NativeBuiltin::ListConcat,
        ("list", "contains") => NativeBuiltin::ListContains,
        ("list", "first") => NativeBuiltin::ListFirst,
        ("list", "get") => NativeBuiltin::ListGet,
        ("list", "index_of") => NativeBuiltin::ListIndexOf,
        ("list", "insert") => NativeBuiltin::ListInsert,
        ("list", "is_empty") => NativeBuiltin::ListIsEmpty,
        ("list", "join") => NativeBuiltin::ListJoin,
        ("list", "last") => NativeBuiltin::ListLast,
        ("list", "len") => NativeBuiltin::ListLen,
        ("list", "pop") => NativeBuiltin::ListPop,
        ("list", "push") => NativeBuiltin::ListPush,
        ("list", "remove_at") => NativeBuiltin::ListRemoveAt,
        ("list", "reverse") => NativeBuiltin::ListReverse,
        ("list", "set") => NativeBuiltin::ListSet,
        ("list", "slice") => NativeBuiltin::ListSlice,
        ("list", "sort") => NativeBuiltin::ListSort,
        _ => return None,
    };
    Some(NativeStraightlineValue::Builtin(builtin))
}
