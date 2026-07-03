#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::util::fast_map::fast_hash_map_new;
use alloc::sync::Arc;

use crate::vm::{RuntimeExport, RuntimeModuleState, copy_runtime_value, import_runtime_export};

use super::*;

#[test]
fn import_runtime_export_copies_mixed_map_object_keys_into_destination_heap() {
    let mut source_heap = HeapStore::new();
    let key = source_heap.alloc(HeapValue::String(Arc::<str>::from("source-key")));
    let mut entries = fast_hash_map_new();
    entries.insert(RuntimeMapKey::Obj(key), RuntimeVal::Int(42));
    let map = source_heap.alloc(HeapValue::Map(TypedMap::Mixed(entries)));
    let state = Arc::new(Mutex::new(RuntimeModuleState::new(source_heap, Vec::new())));
    let export = RuntimeExport::new(RuntimeVal::Obj(map), Arc::clone(&state), Arc::new(Module::default()));
    let mut dest_heap = HeapStore::new();

    let imported = import_runtime_export(&export, &mut dest_heap).expect("use export");

    let RuntimeVal::Obj(imported_map) = imported else {
        panic!("use should return map object");
    };
    let Some(HeapValue::Map(TypedMap::Mixed(entries))) = dest_heap.get(imported_map) else {
        panic!("imported value should be a mixed map");
    };
    let RuntimeMapKey::Obj(imported_key) = entries.keys().next().expect("map key") else {
        panic!("object key should remain object key");
    };
    assert_ne!(*imported_key, imported_map);
    assert!(matches!(
        dest_heap.get(*imported_key),
        Some(HeapValue::String(value)) if value.as_ref() == "source-key"
    ));
}

#[test]
fn copy_runtime_value_copies_mixed_map_object_keys_into_destination_heap() {
    let mut source_heap = HeapStore::new();
    let key = source_heap.alloc(HeapValue::String(Arc::<str>::from("copy-key")));
    let mut entries = fast_hash_map_new();
    entries.insert(RuntimeMapKey::Obj(key), RuntimeVal::Int(7));
    let map = source_heap.alloc(HeapValue::Map(TypedMap::Mixed(entries)));
    let mut dest_heap = HeapStore::new();

    let copied = copy_runtime_value(&RuntimeVal::Obj(map), &source_heap, &mut dest_heap).expect("copy map");

    let RuntimeVal::Obj(copied_map) = copied else {
        panic!("copy should return map object");
    };
    let Some(HeapValue::Map(TypedMap::Mixed(entries))) = dest_heap.get(copied_map) else {
        panic!("copied value should be a mixed map");
    };
    let RuntimeMapKey::Obj(copied_key) = entries.keys().next().expect("map key") else {
        panic!("object key should remain object key");
    };
    assert_ne!(*copied_key, copied_map);
    assert!(matches!(
        dest_heap.get(*copied_key),
        Some(HeapValue::String(value)) if value.as_ref() == "copy-key"
    ));
}
