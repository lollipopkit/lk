use super::*;
use crate::vm::ConstHeapValue;

fn empty_closure(function_index: u32, heap: &mut HeapStore) -> RuntimeVal {
    RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Closure {
        function_index,
        captures: Arc::new(Vec::new()),
    })))
}

fn assert_ok_pair(state: &RuntimeModuleState, result: RuntimeVal, expected: RuntimeVal) {
    let RuntimeVal::Obj(handle) = result else {
        panic!("expected a [ok, value] list, got {result:?}");
    };
    let Some(HeapValue::List(TypedList::Mixed(items))) = state.heap.get(handle) else {
        panic!("expected a Mixed list");
    };
    assert_eq!(items.as_slice(), [RuntimeVal::Bool(true), expected]);
}

fn assert_err_pair(state: &RuntimeModuleState, result: RuntimeVal) -> RuntimeVal {
    let RuntimeVal::Obj(handle) = result else {
        panic!("expected a [ok, value] list, got {result:?}");
    };
    let Some(HeapValue::List(TypedList::Mixed(items))) = state.heap.get(handle) else {
        panic!("expected a Mixed list");
    };
    assert_eq!(items[0], RuntimeVal::Bool(false));
    items[1]
}

/// No `yield` at all: resume runs the body to completion in one step.
#[test]
fn coroutine_without_yield_completes_on_first_resume() {
    let body = Function {
        consts: ConstPool {
            ints: vec![42],
            ..ConstPool::default()
        },
        code: vec![Instr::abx(Opcode::LoadInt, 0, 0), Instr::abc(Opcode::Return, 0, 1, 0)],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![body],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };
    let mut state = RuntimeModuleState::new(HeapStore::new(), Vec::new());
    let closure = empty_closure(0, &mut state.heap);
    let co = create_coroutine_runtime(closure, &mut state.heap).expect("create");
    state.globals = vec![co]; // root it, like a real program's local variable would
    assert_eq!(coroutine_status_runtime(co, &state.heap).unwrap(), "suspended");

    let mut ctx = VmContext::new_without_core_vm_builtins();
    let result = resume_coroutine_runtime(co, &[], &mut state, Some(&module), Some(&mut ctx)).expect("resume");

    assert_ok_pair(&state, result, RuntimeVal::Int(42));
    assert_eq!(coroutine_status_runtime(co, &state.heap).unwrap(), "dead");
}

/// `let x = yield n; return x + 100;` — two-way value passing across a
/// suspend/resume boundary, driven entirely through `Executor::frames`
/// (plan M2.5), not the Rust stack.
#[test]
fn coroutine_yield_then_resume_completes_with_resumed_value() {
    let body = Function {
        consts: ConstPool {
            ints: vec![100],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abc(Opcode::Yield, 0, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::AddInt, 1, 0, 1),
            Instr::abc(Opcode::Return, 1, 1, 0),
        ],
        register_count: 2,
        param_count: 1,
        positional_param_count: 1,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![body],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };
    let mut state = RuntimeModuleState::new(HeapStore::new(), Vec::new());
    let closure = empty_closure(0, &mut state.heap);
    let co = create_coroutine_runtime(closure, &mut state.heap).expect("create");
    state.globals = vec![co]; // root it, like a real program's local variable would
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let first = resume_coroutine_runtime(co, &[RuntimeVal::Int(5)], &mut state, Some(&module), Some(&mut ctx))
        .expect("first resume");
    assert_ok_pair(&state, first, RuntimeVal::Int(5));
    assert_eq!(coroutine_status_runtime(co, &state.heap).unwrap(), "suspended");

    let second = resume_coroutine_runtime(co, &[RuntimeVal::Int(7)], &mut state, Some(&module), Some(&mut ctx))
        .expect("second resume");
    assert_ok_pair(&state, second, RuntimeVal::Int(107));
    assert_eq!(coroutine_status_runtime(co, &state.heap).unwrap(), "dead");

    let err = resume_coroutine_runtime(co, &[], &mut state, Some(&module), Some(&mut ctx))
        .expect_err("resuming a dead coroutine must error");
    assert!(err.to_string().contains("dead"), "unexpected error: {err}");
}

/// An uncaught error inside the coroutine marks it dead and surfaces as
/// `[false, message]`, mirroring `pcall`'s convention.
#[test]
fn coroutine_uncaught_error_marks_dead_and_reports_false() {
    let body = Function {
        consts: ConstPool {
            ints: vec![1, 0],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::DivInt, 0, 0, 1),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![body],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };
    let mut state = RuntimeModuleState::new(HeapStore::new(), Vec::new());
    let closure = empty_closure(0, &mut state.heap);
    let co = create_coroutine_runtime(closure, &mut state.heap).expect("create");
    state.globals = vec![co]; // root it, like a real program's local variable would
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result =
        resume_coroutine_runtime(co, &[], &mut state, Some(&module), Some(&mut ctx)).expect("resume completes");
    let error_value = assert_err_pair(&state, result);
    let RuntimeVal::Obj(handle) = error_value else {
        panic!("expected a heap-allocated error message");
    };
    let Some(HeapValue::String(message)) = state.heap.get(handle) else {
        panic!("expected a String error message");
    };
    assert!(message.contains("divisor is zero"), "unexpected message: {message}");
    assert_eq!(coroutine_status_runtime(co, &state.heap).unwrap(), "dead");
}

/// `yield` used outside any coroutine (a plain direct call) is a catchable
/// runtime error, not a panic or silent corruption.
#[test]
fn yield_outside_coroutine_errors() {
    let function = Function {
        code: vec![Instr::abc(Opcode::Yield, 0, 0, 0), Instr::abc(Opcode::Return, 0, 1, 0)],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let err = execute(&function).expect_err("yield outside a coroutine must error");
    assert!(
        err.to_string().contains("yield used outside a running coroutine"),
        "unexpected error: {err}"
    );
}

/// Decision 4: yielding across a native-re-entry boundary is a runtime
/// error. A coroutine body calls a native that re-enters the VM via
/// `call_runtime_value_runtime` (the same primitive `pcall` uses) on a
/// *separate* closure that tries to `yield` — that nested `Executor` never
/// has `active_coroutine` set, so the `Yield` there must fail cleanly.
#[test]
fn yield_across_native_reentry_boundary_errors() {
    fn call_through(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let callee = args.as_slice()[0];
        let Some((state, ctx, module)) = runtime.state_ctx_module_mut() else {
            bail!("call_through requires full VM state");
        };
        call_runtime_value_runtime(callee, &[], state, module, ctx)
    }

    let yielding_body = Function {
        code: vec![Instr::abc(Opcode::Yield, 0, 0, 0), Instr::abc(Opcode::Return, 0, 1, 0)],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let coroutine_body = Function {
        code: vec![
            Instr::abx(Opcode::LoadNative, 0, 0),
            Instr::abx(Opcode::LoadFunction, 1, 1),
            Instr::abc(Opcode::Call, 0, 0, 1),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![coroutine_body, yielding_body],
        natives: vec![NativeEntry {
            name: "call_through".to_string(),
            arity: 1,
            function: NativeFunction::FullState(call_through),
        }],
        globals: Vec::new(),
        entry: 0,
    };
    let mut state = RuntimeModuleState::new(HeapStore::new(), Vec::new());
    let closure = empty_closure(0, &mut state.heap);
    let co = create_coroutine_runtime(closure, &mut state.heap).expect("create");
    state.globals = vec![co]; // root it, like a real program's local variable would
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result =
        resume_coroutine_runtime(co, &[], &mut state, Some(&module), Some(&mut ctx)).expect("resume completes");
    let error_value = assert_err_pair(&state, result);
    let RuntimeVal::Obj(handle) = error_value else {
        panic!("expected a heap-allocated error message");
    };
    let Some(HeapValue::String(message)) = state.heap.get(handle) else {
        panic!("expected a String error message");
    };
    assert!(
        message.contains("yield used outside a running coroutine"),
        "unexpected message: {message}"
    );
    assert_eq!(coroutine_status_runtime(co, &state.heap).unwrap(), "dead");
}

/// A parked coroutine's own stack is only reachable through `HeapValue::
/// Coroutine`'s GC edges (`val/runtime_model/heap.rs`), not through anything
/// else — this proves a value that's alive *only* in the suspended
/// coroutine's own register file survives a collection that happens while
/// it's parked (between two resumes).
#[test]
fn suspended_coroutine_stack_survives_gc_between_resumes() {
    let body = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from(
                "only-alive-in-parked-coroutine",
            ))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0), // r0 = long string (never yielded/returned directly)
            Instr::abc(Opcode::Yield, 1, 0, 0),      // yields Nil (r1 is Nil), parks with r0 alive only here
            Instr::abc(Opcode::Return, 0, 1, 0),     // proves r0 survived by returning it
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![body],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };
    let mut state = RuntimeModuleState::new(HeapStore::new(), Vec::new());
    let closure = empty_closure(0, &mut state.heap);
    let co = create_coroutine_runtime(closure, &mut state.heap).expect("create");
    state.globals = vec![co]; // root it, like a real program's local variable would
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let first = resume_coroutine_runtime(co, &[], &mut state, Some(&module), Some(&mut ctx)).expect("first resume");
    assert_ok_pair(&state, first, RuntimeVal::Nil);
    assert_eq!(coroutine_status_runtime(co, &state.heap).unwrap(), "suspended");

    // Force a real collection while the coroutine sits parked: root only the
    // coroutine handle itself (as if it were the sole surviving reference —
    // no register/global anywhere holds `r0`'s string directly) plus enough
    // garbage to prove sweep actually runs.
    for _ in 0..8 {
        state.heap.alloc(HeapValue::String(Arc::<str>::from("garbage")));
    }
    state.heap.collect(vec![co_ref(co)]);

    let second = resume_coroutine_runtime(co, &[], &mut state, Some(&module), Some(&mut ctx)).expect("second resume");
    let RuntimeVal::Obj(handle) = second else {
        panic!("expected a [ok, value] list");
    };
    let Some(HeapValue::List(TypedList::Mixed(items))) = state.heap.get(handle) else {
        panic!("expected a Mixed list");
    };
    assert_eq!(items[0], RuntimeVal::Bool(true));
    let RuntimeVal::Obj(string_handle) = items[1] else {
        panic!("expected the surviving string back");
    };
    let Some(HeapValue::String(text)) = state.heap.get(string_handle) else {
        panic!("expected a String — the parked coroutine's stack was not rooted correctly");
    };
    assert_eq!(text.as_ref(), "only-alive-in-parked-coroutine");
}

fn co_ref(value: RuntimeVal) -> HeapRef {
    let RuntimeVal::Obj(handle) = value else {
        panic!("expected an object reference");
    };
    handle
}
