use super::*;
use crate::val::Val;
use crate::{
    stmt::{Program, stmt_parser::StmtParser},
    token::Tokenizer,
    vm::{ModuleFlags, compile_program, encode_module},
};
use once_cell::sync::Lazy;
use std::{
    ffi::CString,
    path::{Path, PathBuf},
    sync::Mutex,
};

static RUNTIME_TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn reset_runtime_state() {
    if let Some(mutex) = RUNTIME_STATE.get() {
        let mut guard = mutex.lock().unwrap();
        *guard = RuntimeState::default();
    }
}

fn decode_for_tests(value: i64) -> Val {
    with_state(|state| state.decode_value(value))
}

#[test]
fn intern_string_roundtrips() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    let text = b"hello";
    let handle = lk_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
    let handle_again = lk_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
    assert_eq!(handle, handle_again);
    assert!(matches!(decode_for_tests(handle), ref v if v.as_str() == Some("hello")));
}

#[test]
fn build_list_and_len() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    let values = [encoding::BOOL_TRUE_VALUE, 42, encoding::NIL_VALUE];
    let list_handle = lk_rt_build_list(values.as_ptr(), values.len() as i64);
    let len = lk_rt_len(list_handle);
    assert_eq!(len, 3);
    match decode_for_tests(list_handle) {
        Val::List(list) => {
            assert_eq!(list.len(), 3);
            assert!(matches!(list[0], Val::Bool(true)));
            assert!(matches!(list[1], Val::Int(42)));
            assert!(matches!(list[2], Val::Nil));
        }
        other => panic!("unexpected value: {other:?}"),
    }
}

#[test]
fn string_int_key_helpers_match_dynamic_map_access() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();

    let prefix = c"b";
    let key = lk_rt_add(lk_rt_intern_string(prefix.as_ptr().cast(), 1), 7);
    let entries = [key, 41];
    let map = lk_rt_build_map(entries.as_ptr(), 1);
    assert_eq!(lk_rt_access_str_int(map, prefix.as_ptr().cast(), 1, 7), 41);
    assert_eq!(lk_rt_map_set_str_int(map, prefix.as_ptr().cast(), 1, 7, 42), map);
    assert_eq!(lk_rt_access(map, key), 42);
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
fn access_list_by_immediate_index_uses_current_handle_contents() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();

    let values = [10, 20, 30];
    let list_handle = lk_rt_build_list(values.as_ptr(), values.len() as i64);
    assert_eq!(lk_rt_access(list_handle, 1), 20);
    assert_eq!(lk_rt_add_access(5, list_handle, 1), 25);
    assert_eq!(lk_rt_sub_access(50, list_handle, 2), 20);
    assert!(matches!(decode_for_tests(lk_rt_access(list_handle, -1)), Val::Nil));
    assert!(matches!(decode_for_tests(lk_rt_access(list_handle, 9)), Val::Nil));
}

#[test]
fn index_string_preserves_ascii_and_unicode_lengths() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();

    let ascii = b"abc";
    let ascii_handle = lk_rt_intern_string(ascii.as_ptr().cast(), ascii.len() as i64);
    let ascii_char = lk_rt_index(ascii_handle, 1);
    assert!(matches!(decode_for_tests(ascii_char), ref v if v.as_str() == Some("b")));
    assert_eq!(lk_rt_len(ascii_char), 1);

    let unicode = "éx";
    let unicode_handle = lk_rt_intern_string(unicode.as_ptr().cast(), unicode.len() as i64);
    let unicode_char = lk_rt_index(unicode_handle, 0);
    assert!(matches!(decode_for_tests(unicode_char), ref v if v.as_str() == Some("é")));
    assert_eq!(lk_rt_len(unicode_char), 2);
    assert_eq!(lk_rt_index_len(unicode_handle, 0), 2);

    let values = [ascii_handle];
    let list_handle = lk_rt_build_list(values.as_ptr(), values.len() as i64);
    assert_eq!(lk_rt_index_len(list_handle, 0), 3);
}

#[test]
fn to_iter_reuses_string_handles_but_keeps_list_snapshot_handle() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();

    let text = b"abc";
    let string_handle = lk_rt_intern_string(text.as_ptr().cast(), text.len() as i64);
    assert_eq!(lk_rt_to_iter(string_handle), string_handle);

    let values = [1, 2, 3];
    let list_handle = lk_rt_build_list(values.as_ptr(), values.len() as i64);
    let iter_handle = lk_rt_to_iter(list_handle);
    assert_ne!(iter_handle, list_handle);
    assert_eq!(lk_rt_len(iter_handle), 3);
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

fn add_one(args: &[Val], _ctx: &mut VmContext) -> Result<Val> {
    let value = args.first().cloned().unwrap_or(Val::Int(0));
    match value {
        Val::Int(i) => Ok(Val::Int(i + 1)),
        _ => Ok(Val::Nil),
    }
}

#[test]
fn call_rust_function() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    let func_handle = with_state(|state| state.encode_value(Val::RustFunction(add_one)));
    let arg = 41i64;
    let result = lk_rt_call(func_handle, &arg, 1, 1);
    assert_eq!(result, 42);
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

fn compile_module_from_path(path: &Path) -> BytecodeModule {
    let src = std::fs::read_to_string(path).expect("module source readable");
    let (tokens, spans) = Tokenizer::tokenize_enhanced_with_spans(&src).expect("tokenize module");
    let mut parser = StmtParser::new_with_spans(&tokens, &spans);
    let program: Program = parser.parse_program_with_enhanced_errors(&src).expect("parse module");
    let func = compile_program(&program);
    let mut module = BytecodeModule::new(func);
    module.flags.insert(ModuleFlags::CONST_FOLDED);
    module
}

#[test]
fn apply_imports_registers_bundled_module() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    lk_rt_begin_session();

    let workspace = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .to_path_buf();
    let examples_dir = workspace.join("examples");
    let search_path = CString::new("examples").unwrap();
    lk_rt_register_search_path(search_path.as_ptr(), search_path.as_bytes().len() as i64);

    let fib_path = examples_dir.join("fib.lk");
    let fib_module = compile_module_from_path(&fib_path);
    let fib_bytes = encode_module(&fib_module).expect("encode module");
    let fib_path_rel = CString::new("examples/fib.lk").unwrap();
    let _ = lk_rt_register_bundled_module(
        fib_path_rel.as_ptr(),
        fib_path_rel.as_bytes().len() as i64,
        fib_bytes.as_ptr(),
        fib_bytes.len() as i64,
    );

    let imports_json = CString::new("[{\"File\":{\"path\":\"examples/fib\"}}]").unwrap();
    let _ = lk_rt_register_imports(imports_json.as_ptr(), imports_json.as_bytes().len() as i64);
    let apply = lk_rt_apply_imports();
    assert_eq!(apply, 0, "apply imports succeeds");

    let fib_name = CString::new("fib").unwrap();
    let fib_handle = lk_rt_intern_string(fib_name.as_ptr(), fib_name.as_bytes().len() as i64);
    let fib_value = lk_rt_load_global(fib_handle);
    assert_ne!(fib_value, encoding::NIL_VALUE, "fib module is loaded");
}
