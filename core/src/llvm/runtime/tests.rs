use super::*;
use crate::{
    stmt::{ImportItem, ImportSource, ImportStmt, stmt_parser::StmtParser},
    token::Tokenizer,
    val::{CallableValue, HeapValue, RuntimeMapKey},
    vm::Compiler32,
};
use once_cell::sync::Lazy;
use std::sync::Mutex;

static RUNTIME_TEST_LOCK: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

fn reset_runtime_state() {
    if let Some(mutex) = RUNTIME_STATE.get() {
        let mut guard = mutex.lock().unwrap();
        *guard = RuntimeState::default();
    }
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
fn module32_json_runtime_entry_executes_artifact() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    lk_rt_begin_session();

    let tokens = Tokenizer::tokenize("return 42;").expect("tokens");
    let program = StmtParser::new(&tokens).parse_program().expect("program");
    let module = Compiler32::compile_module(&program).expect("compile module");
    let artifact = Module32Artifact::new(Vec::new(), &module).expect("artifact");
    let json = artifact.to_json_string().expect("json");

    let status = lk_rt_run_module32_json(json.as_ptr().cast(), json.len() as i64);
    assert_eq!(status, 0);
}

#[test]
fn module32_json_runtime_entry_rejects_invalid_artifact() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();
    lk_rt_begin_session();

    let invalid = b"{\"format\":\"not.lk.module32\"}";
    let status = lk_rt_run_module32_json(invalid.as_ptr().cast(), invalid.len() as i64);
    assert_eq!(status, -1);
}

#[test]
fn native_import_replay_imports_file_module_and_items() {
    let _guard = RUNTIME_TEST_LOCK.lock().unwrap();
    reset_runtime_state();

    let mut state = RuntimeState::default();
    state.pending_search_paths.push("..".to_string());
    state.pending_imports = vec![
        ImportStmt::File {
            path: "examples/fib".to_string(),
        },
        ImportStmt::Items {
            items: vec![ImportItem {
                name: "iterative".to_string(),
                alias: Some("fib_iter".to_string()),
            }],
            source: ImportSource::File("examples/fib".to_string()),
        },
    ];

    state.apply_pending_native_only().expect("native imports");

    let fib = state
        .artifact_globals
        .get("fib")
        .expect("file module global is replayed");
    let RuntimeVal::Obj(fib_handle) = fib else {
        panic!("file module import should be a heap map");
    };
    let Some(HeapValue::Map(fib_map)) = state.heap.get(*fib_handle) else {
        panic!("file module import should be a heap map");
    };
    assert!(matches!(
        fib_map.get(&RuntimeMapKey::String("iterative".into())),
        Some(RuntimeVal::Obj(_))
    ));

    let fib_iter = state
        .artifact_globals
        .get("fib_iter")
        .expect("item import global is replayed");
    let RuntimeVal::Obj(callable_handle) = fib_iter else {
        panic!("item import should be a runtime callable object");
    };
    assert!(matches!(
        state.heap.get(*callable_handle),
        Some(HeapValue::Callable(CallableValue::Runtime32(_)))
    ));
}
