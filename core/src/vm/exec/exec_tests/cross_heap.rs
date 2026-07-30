#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::util::fast_map::fast_hash_map_new;
use alloc::sync::Arc;

use crate::vm::{RuntimeExport, RuntimeModuleState, copy_runtime_value, import_runtime_export};

use super::*;

/// A map key crosses heaps as itself: `RuntimeMapKey` carries no heap handle —
/// a long string is an `Arc<str>` held inline, and a container cannot be a key
/// at all. These used to build an `Obj` key, a shape no program could produce,
/// and assert that the translation followed the handle across.
#[test]
fn a_long_string_map_key_survives_both_crossings_as_itself() {
    let mut source_heap = HeapStore::new();
    let mut entries = fast_hash_map_new();
    entries.insert(
        RuntimeMapKey::String(Arc::<str>::from("a key too long to live inline")),
        RuntimeVal::Int(42),
    );
    let map = source_heap.alloc(HeapValue::Map(TypedMap::Mixed(entries)));

    let mut copy_heap = HeapStore::new();
    let copied = copy_runtime_value(&RuntimeVal::Obj(map), &source_heap, &mut copy_heap).expect("copy map");

    let state = Arc::new(Mutex::new(RuntimeModuleState::new(source_heap, Vec::new())));
    let export = RuntimeExport::new(RuntimeVal::Obj(map), Arc::clone(&state), Arc::new(Module::default()));
    let mut import_heap = HeapStore::new();
    let imported = import_runtime_export(&export, &mut import_heap).expect("use export");

    for (value, heap) in [(copied, &copy_heap), (imported, &import_heap)] {
        let RuntimeVal::Obj(handle) = value else {
            panic!("expected a map object");
        };
        let Some(HeapValue::Map(TypedMap::Mixed(entries))) = heap.get(handle) else {
            panic!("expected a mixed map");
        };
        let RuntimeMapKey::String(key) = entries.keys().next().expect("map key") else {
            panic!("a long string key stays a string key");
        };
        assert_eq!(key.as_ref(), "a key too long to live inline");
    }
}

/// A function leaving its module says what the program did, not what the copy
/// function lacked.
///
/// The text was "cannot copy closure without module context" — a sentence about
/// a parameter of `copy_runtime_value`. What a user wrote was
/// `apply(double, 5)`, where `apply` came from another file; the same call works
/// when both sit in one. A bare closure is an index into *its own* module's
/// function table, so it cannot be read anywhere else — that is the fact worth
/// stating, together with the two ways around it.
///
/// The limitation stays a run-time one on purpose: the export direction already
/// promotes a crossing closure to a module-carrying callable, and the argument
/// direction is meant to follow. Turning it into a check-time rule would codify
/// something we intend to lift.
#[test]
fn a_function_leaving_its_module_is_refused_in_the_programs_terms() {
    let mut source_heap = HeapStore::new();
    let closure = source_heap.alloc(HeapValue::Callable(CallableValue::Closure {
        function_index: 0,
        captures: Arc::new(Vec::new()),
    }));

    let mut dest_heap = HeapStore::new();
    let error = copy_runtime_value(&RuntimeVal::Obj(closure), &source_heap, &mut dest_heap)
        .expect_err("a closure has no meaning in another module");

    let message = format!("{error:#}");
    assert!(
        message.contains("cannot be passed out of the module that defined it"),
        "{message}"
    );
    assert!(
        message.contains("Move the function into the module that calls it"),
        "{message}"
    );
}
