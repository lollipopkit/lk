use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::vm::analysis::{PerfCallFact, PerfCallTargetKind};

#[test]
fn execute_module_calls_closure_function() {
    let callee = Function {
        consts: ConstPool::default(),
        code: vec![Instr::abc(Opcode::AddInt, 2, 0, 1), Instr::abc(Opcode::Return, 2, 1, 0)],
        register_count: 3,
        param_count: 2,
        positional_param_count: 2,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let entry = Function {
        consts: ConstPool {
            ints: vec![11, 31],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadInt, 2, 1),
            Instr::abc(Opcode::Call, 0, 0, 2),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.stack[1], RuntimeVal::Nil);
    assert_eq!(result.state.stack[2], RuntimeVal::Nil);
}

#[test]
fn execute_module_uses_call_shape_fact_for_call_window() {
    let callee = Function {
        consts: ConstPool::default(),
        code: vec![Instr::abc(Opcode::AddInt, 2, 0, 1), Instr::abc(Opcode::Return, 2, 1, 0)],
        register_count: 3,
        param_count: 2,
        positional_param_count: 2,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let mut entry = Function {
        consts: ConstPool {
            ints: vec![11, 31],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadInt, 2, 1),
            Instr::abc(Opcode::Call, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    entry.performance.set_call_fact(
        3,
        PerfCallFact {
            call_base: 0,
            positional_count: 2,
            named_count: 0,
            target_kind: crate::vm::analysis::PerfCallTargetKind::Closure,
        },
    );
    let module = Module {
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_module_caches_call_shape_without_static_fact() {
    let callee = Function {
        consts: ConstPool::default(),
        code: vec![Instr::abc(Opcode::AddInt, 2, 0, 1), Instr::abc(Opcode::Return, 2, 1, 0)],
        register_count: 3,
        param_count: 2,
        positional_param_count: 2,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let entry = Function {
        consts: ConstPool {
            ints: vec![11, 31],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadInt, 2, 1),
            Instr::abc(Opcode::Call, 0, 0, 2),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };
    assert!(module.functions[0].performance.call_site(3).is_none());

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(module.functions[0].performance.call_site(3).is_none());
    assert_eq!(
        result.state.inline_caches.call(3),
        Some(PerfCallFact {
            call_base: 0,
            positional_count: 2,
            named_count: 0,
            target_kind: PerfCallTargetKind::Closure,
        })
    );
}

#[test]
fn execute_module_caches_named_call_shape_without_static_fact() {
    let callee = Function {
        consts: ConstPool::default(),
        code: vec![Instr::abc(Opcode::AddInt, 2, 0, 1), Instr::abc(Opcode::Return, 2, 1, 0)],
        register_count: 3,
        param_count: 2,
        positional_param_count: 1,
        param_names: vec![Arc::<str>::from("x"), Arc::<str>::from("y")],
        capture_count: 0,
        ..Function::default()
    };
    let entry = Function {
        consts: ConstPool {
            ints: vec![40, 2],
            strings: vec!["y".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::LoadString, 2, 0),
            Instr::abx(Opcode::LoadInt, 3, 1),
            Instr::abx(Opcode::CallNamed, 0, (1 << 7) | 1),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };
    assert!(module.functions[0].performance.call_site(4).is_none());

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(module.functions[0].performance.call_site(4).is_none());
    assert_eq!(result.state.stack[1], RuntimeVal::Nil);
    assert_eq!(result.state.stack[2], RuntimeVal::Nil);
    assert_eq!(result.state.stack[3], RuntimeVal::Nil);
    assert_eq!(
        result.state.inline_caches.call(4),
        Some(PerfCallFact {
            call_base: 0,
            positional_count: 1,
            named_count: 1,
            target_kind: PerfCallTargetKind::Closure,
        })
    );
}

#[test]
fn execute_module_calls_closure_with_captured_value() {
    let callee = Function {
        consts: ConstPool::default(),
        code: vec![
            Instr::abx(Opcode::LoadCapture, 1, 0),
            Instr::abc(Opcode::AddInt, 2, 0, 1),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 1,
        positional_param_count: 1,
        param_names: Vec::new(),
        capture_count: 1,
        ..Function::default()
    };
    let entry = Function {
        consts: ConstPool {
            ints: vec![40, 2],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::MakeClosure, 0, 1, 1),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::Call, 0, 0, 1),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_module_reuses_shared_stack_for_repeated_closure_calls() {
    let callee = Function {
        consts: ConstPool {
            ints: vec![1],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::AddInt, 2, 0, 1),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 1,
        positional_param_count: 1,
        param_names: vec![Arc::<str>::from("x")],
        capture_count: 0,
        ..Function::default()
    };
    let entry = Function {
        consts: ConstPool {
            ints: vec![0, 10, 20],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::Call, 0, 0, 1),
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abx(Opcode::LoadInt, 1, 1),
            Instr::abc(Opcode::Call, 0, 0, 1),
            Instr::abx(Opcode::LoadFunction, 0, 1),
            Instr::abx(Opcode::LoadInt, 1, 2),
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
        functions: vec![entry, callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module(&module).expect("execute repeated closure calls");

    assert_eq!(result.returns, vec![RuntimeVal::Int(21)]);
    assert_eq!(result.state.stack_top, 2);
    assert_eq!(result.state.stack.len(), 5);
}

#[test]
fn runtime_value_closure_call_uses_active_shared_stack_window() {
    fn invoke_global_closure(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> anyhow::Result<RuntimeVal> {
        let callee = runtime
            .globals()
            .first()
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("missing closure global"))?;
        let Some((state, ctx, module)) = runtime.parts_mut() else {
            return Err(anyhow::anyhow!("test native requires full runtime state"));
        };
        call_runtime_value_runtime(callee, args.as_slice(), state, module, ctx)
    }

    let callee = Function {
        consts: ConstPool {
            ints: vec![2],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::AddInt, 2, 0, 1),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 1,
        positional_param_count: 1,
        param_names: vec![Arc::<str>::from("x")],
        capture_count: 0,
        ..Function::default()
    };
    let entry = Function {
        consts: ConstPool {
            ints: vec![40],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadNative, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abc(Opcode::Call, 0, 0, 1),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![entry, callee],
        natives: vec![NativeEntry {
            name: "invoke_global_closure".to_string(),
            arity: 1,
            function: NativeFunction::FullState(invoke_global_closure),
        }],
        globals: vec![GlobalSlot { name: "f".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let closure = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Closure {
        function_index: 1,
        captures: Arc::new(Vec::new()),
    })));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module_with_globals_heap_and_ctx(&module, vec![closure], heap, &mut ctx)
        .expect("execute native-mediated closure call");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.stack_top, 4);
    assert_eq!(result.state.stack.len(), 7);
}
