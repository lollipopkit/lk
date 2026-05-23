use super::*;
use once_cell::sync::Lazy;
use std::{ffi::CString, sync::Mutex};

static RUNTIME_TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn reset_runtime_state() {
    if let Some(mutex) = RUNTIME_STATE.get() {
        let mut guard = mutex.lock().unwrap();
        *guard = RuntimeState::default();
    }
}

fn runtime_string_for_tests(value: i64) -> Option<String> {
    with_state(|state| {
        let decoded = state.decode_value(value);
        state.runtime_string(&decoded).map(ToOwned::to_owned)
    })
}

#[test]
fn intern_string_roundtrips() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    let text = b"hello";
    let handle = lk_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
    let handle_again = lk_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
    assert_eq!(handle, handle_again);
    assert_eq!(runtime_string_for_tests(handle).as_deref(), Some("hello"));
}

#[test]
fn arithmetic_helpers_preserve_dynamic_semantics() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();

    assert_eq!(lk_rt_add(40, 2), 42);
    assert_eq!(lk_rt_sub(40, 2), 38);
    assert_eq!(lk_rt_mul(6, 7), 42);
    assert_eq!(lk_rt_div(84, 2), 42);
    assert_eq!(lk_rt_mod(85, 43), 42);
    assert_eq!(lk_rt_add(encoding::BOOL_TRUE_VALUE, 2), encoding::NIL_VALUE);
}

#[test]
fn to_string_uses_scalar_runtime_handles() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();

    let rendered = lk_rt_to_string(42);
    assert_eq!(runtime_string_for_tests(rendered).as_deref(), Some("42"));
}

#[test]
fn define_and_load_global() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    let name_bytes = b"g";
    let name_handle = lk_rt_intern_string(name_bytes.as_ptr().cast(), 1);
    lk_rt_define_global(name_handle, encoding::BOOL_TRUE_VALUE);
    let loaded = lk_rt_load_global(name_handle);
    assert_eq!(loaded, encoding::BOOL_TRUE_VALUE);
}

#[test]
fn stdlib_module_names_are_collected_from_module_imports() {
    let imports = vec![
        ImportStmt::Module {
            module: "math".to_string(),
        },
        ImportStmt::Items {
            items: Vec::new(),
            source: ImportSource::Module("json".to_string()),
        },
        ImportStmt::Namespace {
            alias: "m".to_string(),
            source: ImportSource::File("local.lk".to_string()),
        },
        ImportStmt::ModuleAlias {
            module: "math".to_string(),
            alias: "m".to_string(),
        },
    ];

    assert_eq!(
        stdlib_module_names_from_imports(&imports),
        vec!["math".to_string(), "json".to_string()]
    );
}

#[test]
fn concurrency_globals_are_registered_only_for_concurrency_imports() {
    assert!(!imports_need_concurrency_globals(&[ImportStmt::Module {
        module: "math".to_string(),
    }]));
    assert!(imports_need_concurrency_globals(&[ImportStmt::Module {
        module: "chan".to_string(),
    }]));
}

#[test]
fn bundled_lkb_registration_is_disabled_during_instr32_migration() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    lk_rt_begin_session();

    let fib_path_rel = CString::new("examples/fib.lk").unwrap();
    let bytes = b"LKB";
    let registered = lk_rt_register_bundled_module(
        fib_path_rel.as_ptr(),
        fib_path_rel.as_bytes().len() as i64,
        bytes.as_ptr(),
        bytes.len() as i64,
    );
    assert_eq!(registered, -1, "bundled LKB registration must stay disabled");
}
