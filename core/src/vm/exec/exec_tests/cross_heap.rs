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
