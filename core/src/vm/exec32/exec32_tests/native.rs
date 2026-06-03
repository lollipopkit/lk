use super::*;
use crate::vm::analysis::PerfGlobalFact;
#[test]
fn execute_module32_calls_native_function_with_same_call_opcode() {
    fn native_add(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let [RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)] = args.as_slice() else {
            bail!("native_add expects two ints");
        };
        Ok(RuntimeVal::Int(lhs + rhs))
    }

    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![13, 29],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadNative, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::LoadInt, 2, 1),
            Instr32::abc(Opcode32::Call, 0, 0, 2),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry],
        natives: vec![NativeEntry32 {
            name: "native_add".to_string(),
            arity: 2,
            function: NativeFunction32::Plain(native_add),
        }],
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module32(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.stack[1], RuntimeVal::Nil);
    assert_eq!(result.state.stack[2], RuntimeVal::Nil);
}

#[test]
fn execute_module32_collects_after_native_heap_allocation() {
    fn native_alloc_dead(_args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("native-dead")));
        Ok(RuntimeVal::Nil)
    }

    let entry = Function32 {
        code: vec![
            Instr32::abx(Opcode32::LoadNative, 0, 0),
            Instr32::abc(Opcode32::Call, 0, 0, 0),
            Instr32::abc(Opcode32::Nop, 0, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry],
        natives: vec![NativeEntry32 {
            name: "native_alloc_dead".to_string(),
            arity: 0,
            function: NativeFunction32::Plain(native_alloc_dead),
        }],
        globals: Vec::new(),
        entry: 0,
    };
    let mut heap = HeapStore::new();
    heap.set_gc_threshold(1);

    let result = Executor32::new(1)
        .run_module_with_globals_and_heap(&module, Vec::new(), heap)
        .expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Nil]);
    assert_eq!(result.state.heap.len(), 0);
    assert!(!result.state.heap.should_collect());
}

#[test]
fn execute_module32_calls_full_state_native_with_named_args() {
    fn full_state_clamp(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        if runtime.state_ctx_module_mut().is_none() {
            bail!("full_state_clamp requires active runtime state");
        }
        let [RuntimeVal::Int(value)] = args.as_slice() else {
            bail!("full_state_clamp expects one positional int");
        };
        let mut min = 0;
        let mut max = 100;
        let mut saw_min = false;
        let mut saw_max = false;
        args.try_for_each_named(runtime.heap(), |name, value| {
            let RuntimeVal::Int(value) = value else {
                bail!("{name} must be int");
            };
            match name {
                "min" if !saw_min => {
                    saw_min = true;
                    min = *value;
                }
                "max" if !saw_max => {
                    saw_max = true;
                    max = *value;
                }
                "min" | "max" => bail!("duplicate named argument {name}"),
                other => bail!("unknown named argument {other}"),
            }
            Ok(())
        })?;
        Ok(RuntimeVal::Int((*value).clamp(min, max)))
    }

    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![52, 40, 50],
            strings: vec!["min".to_string(), "max".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadNative, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::LoadString, 2, 0),
            Instr32::abx(Opcode32::LoadInt, 3, 1),
            Instr32::abx(Opcode32::LoadString, 4, 1),
            Instr32::abx(Opcode32::LoadInt, 5, 2),
            Instr32::abx(Opcode32::CallNamed, 0, (2 << 7) | 1),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 6,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry],
        natives: vec![NativeEntry32 {
            name: "full_state_clamp".to_string(),
            arity: 1,
            function: NativeFunction32::FullState(full_state_clamp),
        }],
        globals: Vec::new(),
        entry: 0,
    };

    let result = execute_module32(&module).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(50)]);
    assert_eq!(result.state.stack[1], RuntimeVal::Nil);
    assert_eq!(result.state.stack[2], RuntimeVal::Nil);
    assert_eq!(result.state.stack[3], RuntimeVal::Nil);
    assert_eq!(result.state.stack[4], RuntimeVal::Nil);
    assert_eq!(result.state.stack[5], RuntimeVal::Nil);
}

#[test]
fn execute_module32_calls_runtime32_callable_from_heap() {
    let callee = Function32 {
        consts: ConstPool32 {
            ints: vec![40],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::AddInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 1,
        positional_param_count: 1,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32::single(callee));
    let callable = RuntimeCallable32::with_state(
        Arc::clone(&callee_module),
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(RuntimeModuleState32::new(HeapStore::new(), Vec::new()))),
    );

    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![2],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abc(Opcode32::Call, 0, 0, 1),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 2,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let caller_module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "f".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module32_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx)
        .expect("call runtime32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.stack[1], RuntimeVal::Nil);
}

#[test]
fn direct_runtime_closure_call_restores_state_after_arg_error() {
    let callee = Function32 {
        code: vec![Instr32::abc(Opcode32::Return, 0, 1, 0)],
        register_count: 2,
        param_count: 1,
        positional_param_count: 1,
        param_names: vec!["value".into()],
        ..Function32::default()
    };
    let module = Module32::single(callee);
    let mut state = RuntimeModuleState32::default();
    state.stack_top = 1;
    state.stack.resize(1, RuntimeVal::Nil);
    state.stack[0] = RuntimeVal::Int(99);
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::Closure {
        function_index: 0,
        captures: Arc::new(Vec::new()),
    })));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_value32_runtime(callable.clone(), &[], &mut state, Some(&module), Some(&mut ctx))
        .expect_err("arity error");

    assert!(err.to_string().contains("Function expects 1 positional arguments"));
    assert!(matches!(
        state.heap.get(match callable {
            RuntimeVal::Obj(handle) => handle,
            _ => unreachable!(),
        }),
        Some(HeapValue::Callable(_))
    ));
    assert_eq!(state.stack_top, 1);
    assert_eq!(state.stack[0], RuntimeVal::Int(99));
}

#[test]
fn direct_runtime_closure_named_call_restores_state_after_named_error() {
    let callee = Function32 {
        code: vec![Instr32::abc(Opcode32::Return, 0, 1, 0)],
        register_count: 2,
        param_count: 1,
        positional_param_count: 0,
        param_names: vec!["value".into()],
        ..Function32::default()
    };
    let module = Module32::single(callee);
    let mut state = RuntimeModuleState32::default();
    state.stack_top = 1;
    state.stack.resize(1, RuntimeVal::Nil);
    state.stack[0] = RuntimeVal::Int(77);
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::Closure {
        function_index: 0,
        captures: Arc::new(Vec::new()),
    })));
    let named = state.heap.alloc(HeapValue::String("not-a-map".into()));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_value32_runtime_named_map(
        callable.clone(),
        &[],
        Some(named),
        &mut state,
        Some(&module),
        Some(&mut ctx),
    )
    .expect_err("named map error");

    assert!(err.to_string().contains("named arguments must be a map"));
    assert!(matches!(
        state.heap.get(match callable {
            RuntimeVal::Obj(handle) => handle,
            _ => unreachable!(),
        }),
        Some(HeapValue::Callable(_))
    ));
    assert!(matches!(
        state.heap.get(named),
        Some(HeapValue::String(value)) if value.as_ref() == "not-a-map"
    ));
    assert_eq!(state.stack_top, 1);
    assert_eq!(state.stack[0], RuntimeVal::Int(77));
}

#[test]
fn direct_full_state_native_named_map_uses_heap_map_source() {
    fn full_state_named(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        if runtime.state_ctx_module_mut().is_none() {
            bail!("full_state_named requires active runtime state");
        }
        let [RuntimeVal::Int(value)] = args.as_slice() else {
            bail!("full_state_named expects one positional int");
        };
        let mut increment = 0;
        args.try_for_each_named(runtime.heap(), |name, value| {
            let RuntimeVal::Int(value) = value else {
                bail!("{name} must be int");
            };
            match name {
                "increment" => increment = *value,
                other => bail!("unknown named argument {other}"),
            }
            Ok(())
        })?;
        Ok(RuntimeVal::Int(value + increment))
    }

    let mut state = RuntimeModuleState32::default();
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative32 {
        name: Arc::<str>::from("full_state_named"),
        arity: 1,
        function: NativeFunction32::FullState(full_state_named),
    })));
    let named = state
        .heap
        .alloc(HeapValue::Map(TypedMap::StringInt(std::collections::BTreeMap::from([
            (Arc::<str>::from("increment"), 37),
        ]))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = call_runtime_value32_runtime_named_map(
        callable.clone(),
        &[RuntimeVal::Int(5)],
        Some(named),
        &mut state,
        None,
        Some(&mut ctx),
    )
    .expect("full state native named map");

    assert_eq!(result, RuntimeVal::Int(42));
    assert!(matches!(
        state.heap.get(named),
        Some(HeapValue::Map(TypedMap::StringInt(values)))
            if values.get("increment").copied() == Some(37)
    ));
    assert!(matches!(
        state.heap.get(match callable {
            RuntimeVal::Obj(handle) => handle,
            _ => unreachable!(),
        }),
        Some(HeapValue::Callable(_))
    ));
}

#[test]
fn direct_runtime_native_collects_after_heap_allocation() {
    fn native_alloc_live(_args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("native-dead")));
        let live = runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("native-live")));
        Ok(RuntimeVal::Obj(live))
    }

    let mut state = RuntimeModuleState32::default();
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative32 {
        name: Arc::<str>::from("native_alloc_live"),
        arity: 0,
        function: NativeFunction32::Plain(native_alloc_live),
    })));
    state.heap.set_gc_threshold(1);
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = call_runtime_value32_runtime(callable.clone(), &[], &mut state, None, Some(&mut ctx))
        .expect("direct runtime native");

    let RuntimeVal::Obj(live) = result else {
        panic!("native should return live heap object");
    };
    assert!(matches!(
        state.heap.get(live),
        Some(HeapValue::String(value)) if value.as_ref() == "native-live"
    ));
    assert!(state.heap.get(HeapRef::new(1)).is_none());
    assert!(matches!(
        state.heap.get(match callable {
            RuntimeVal::Obj(handle) => handle,
            _ => unreachable!(),
        }),
        Some(HeapValue::Callable(_))
    ));
    assert!(!state.heap.should_collect());
}

#[test]
fn direct_runtime_native_collects_after_heap_allocation_error() {
    fn native_alloc_then_error(_args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("native-error-dead")));
        bail!("native failed after allocation");
    }

    let mut state = RuntimeModuleState32::default();
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative32 {
        name: Arc::<str>::from("native_alloc_then_error"),
        arity: 0,
        function: NativeFunction32::Plain(native_alloc_then_error),
    })));
    state.heap.set_gc_threshold(1);
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_value32_runtime(callable.clone(), &[], &mut state, None, Some(&mut ctx))
        .expect_err("direct runtime native should fail");

    assert!(err.to_string().contains("native failed after allocation"));
    assert!(state.heap.get(HeapRef::new(1)).is_none());
    assert!(matches!(
        state.heap.get(match callable {
            RuntimeVal::Obj(handle) => handle,
            _ => unreachable!(),
        }),
        Some(HeapValue::Callable(_))
    ));
    assert!(!state.heap.should_collect());
}

#[test]
fn execute_module32_uses_global_slot_fact_for_get_and_set() {
    let mut entry = Function32 {
        consts: ConstPool32 {
            ints: vec![42],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::SetGlobal, 1, 0),
            Instr32::abx(Opcode32::GetGlobal, 2, 0),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    entry.performance.set_global_fact(
        0,
        PerfGlobalFact {
            slot: 1,
            move_source: false,
        },
    );
    entry.performance.set_global_fact(
        2,
        PerfGlobalFact {
            slot: 1,
            move_source: false,
        },
    );
    entry.performance.set_global_fact(
        3,
        PerfGlobalFact {
            slot: 1,
            move_source: false,
        },
    );
    let module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![
            GlobalSlot32 { name: "unused".into() },
            GlobalSlot32 { name: "answer".into() },
        ],
        entry: 0,
    };

    let result =
        execute_module32_with_globals(&module, vec![RuntimeVal::Int(0), RuntimeVal::Int(40)]).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.globals, vec![RuntimeVal::Int(0), RuntimeVal::Int(42)]);
}

#[test]
fn execute_module32_set_global_move_fact_consumes_source_register() {
    let mut entry = Function32 {
        consts: ConstPool32 {
            heap_values: vec![ConstHeapValue32::LongString(Arc::<str>::from("stored-global-value"))],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadHeapConst, 0, 0),
            Instr32::abx(Opcode32::SetGlobal, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    entry.performance.set_global_fact(
        1,
        PerfGlobalFact {
            slot: 0,
            move_source: true,
        },
    );
    let module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "stored".into() }],
        entry: 0,
    };

    let result = execute_module32_with_globals(&module, vec![RuntimeVal::Nil]).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Nil]);
    assert!(matches!(result.state.globals[0], RuntimeVal::Obj(_)));
}

#[test]
fn execute_module32_set_global_without_move_fact_clones_source_register() {
    let entry = Function32 {
        consts: ConstPool32 {
            heap_values: vec![ConstHeapValue32::LongString(Arc::<str>::from("stored-global-value"))],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadHeapConst, 0, 0),
            Instr32::abx(Opcode32::SetGlobal, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "stored".into() }],
        entry: 0,
    };

    let result = execute_module32_with_globals(&module, vec![RuntimeVal::Nil]).expect("execute module");

    assert!(matches!(result.returns[0], RuntimeVal::Obj(_)));
    assert!(matches!(result.state.globals[0], RuntimeVal::Obj(_)));
}

#[test]
fn execute_module32_falls_back_to_instr_global_slot_without_fact() {
    let entry = Function32 {
        consts: ConstPool32::default(),
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "answer".into() }],
        entry: 0,
    };

    let result = execute_module32_with_globals(&module, vec![RuntimeVal::Int(42)]).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.inline_caches.global(0), Some(0));
}

#[test]
fn execute32_caller_handler_catches_raise_from_runtime32_callable() {
    let callee = Function32 {
        consts: ConstPool32 {
            strings: vec!["boom".into()],
            ..ConstPool32::default()
        },
        code: vec![Instr32::abx(Opcode32::Raise, 0, 0)],
        register_count: 1,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32 {
        functions: vec![callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    });
    let callable = RuntimeCallable32::with_state(
        callee_module,
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(RuntimeModuleState32::new(HeapStore::new(), Vec::new()))),
    );
    let entry = Function32 {
        code: vec![
            Instr32::as_bx(Opcode32::TryBegin, 0, 3),
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abc(Opcode32::Call, 0, 0, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 1,
        ..Function32::default()
    };
    let caller_module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "f".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module32_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx)
        .expect("caller handler catches runtime32 raise");
    let RuntimeVal::Obj(handle) = result.returns.first().expect("return") else {
        panic!("handler return should be error object");
    };
    let Some(HeapValue::ErrorVal(error)) = result.state.heap.get(*handle) else {
        panic!("handler return should be ErrorVal");
    };

    assert_eq!(error.message.as_ref(), "boom");
}

#[test]
fn execute_module32_calls_runtime32_callable_with_named_args() {
    let callee = Function32 {
        code: vec![
            Instr32::abc(Opcode32::AddInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 2,
        positional_param_count: 1,
        param_names: vec![Arc::<str>::from("x"), Arc::<str>::from("y")],
        capture_count: 0,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32 {
        functions: vec![callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    });
    let callable = RuntimeCallable32::with_state(
        Arc::clone(&callee_module),
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(RuntimeModuleState32::new(HeapStore::new(), Vec::new()))),
    );

    let entry = Function32 {
        consts: ConstPool32 {
            ints: vec![40, 2],
            strings: vec!["y".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::GetGlobal, 0, 0),
            Instr32::abx(Opcode32::LoadInt, 1, 0),
            Instr32::abx(Opcode32::LoadString, 2, 0),
            Instr32::abx(Opcode32::LoadInt, 3, 1),
            Instr32::abx(Opcode32::CallNamed, 0, (1 << 7) | 1),
            Instr32::abc(Opcode32::Return, 0, 1, 0),
        ],
        register_count: 4,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let caller_module = Module32 {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "f".into() }],
        entry: 0,
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime32(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module32_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx)
        .expect("call runtime32 named");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn runtime32_callable_error_keeps_shared_module_state() {
    let callee = Function32 {
        consts: ConstPool32 {
            ints: vec![41],
            strings: vec!["boom".to_string()],
            ..ConstPool32::default()
        },
        code: vec![
            Instr32::abx(Opcode32::LoadInt, 0, 0),
            Instr32::abx(Opcode32::SetGlobal, 0, 0),
            Instr32::abx(Opcode32::Raise, 0, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32 {
        functions: vec![callee],
        natives: Vec::new(),
        globals: vec![GlobalSlot32 { name: "counter".into() }],
        entry: 0,
    });
    let callable = RuntimeCallable32::with_state(
        Arc::clone(&callee_module),
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(RuntimeModuleState32::new(
            HeapStore::new(),
            vec![RuntimeVal::Int(1)],
        ))),
    );
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_callable32_test(&callable, &[], &mut ctx).expect_err("call should raise");

    assert!(err.to_string().contains("boom"));
    let state = callable.state.lock().expect("callable state");
    assert_eq!(state.globals, vec![RuntimeVal::Int(41)]);
    assert_eq!(
        state.stack_top(),
        0,
        "direct runtime callable errors must not leave their callee frame active"
    );
}

#[test]
fn runtime32_callable_native_error_collects_pending_heap_allocations() {
    fn native_alloc_then_error(_args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("runtime-callable-native-dead")));
        bail!("runtime native failed after allocation");
    }

    let callee = Function32 {
        code: vec![
            Instr32::abx(Opcode32::LoadNative, 0, 0),
            Instr32::abc(Opcode32::Call, 0, 0, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let callee_module = Arc::new(Module32 {
        functions: vec![callee],
        natives: vec![NativeEntry32 {
            name: "native_alloc_then_error".to_string(),
            arity: 0,
            function: NativeFunction32::Plain(native_alloc_then_error),
        }],
        globals: Vec::new(),
        entry: 0,
    });
    let mut state = RuntimeModuleState32::new(HeapStore::new(), Vec::new());
    state.heap.set_gc_threshold(1);
    let callable = RuntimeCallable32::with_state(
        Arc::clone(&callee_module),
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(state)),
    );
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_callable32_test(&callable, &[], &mut ctx).expect_err("native should fail");

    assert!(err.to_string().contains("runtime native failed after allocation"));
    let state = callable.state.lock().expect("callable state");
    assert!(state.heap.get(HeapRef::new(1)).is_none());
    assert!(!state.heap.should_collect());
    assert_eq!(state.stack_top(), 0);
}

#[test]
fn direct_runtime_callable_restores_shared_state_stack_top() {
    let callee = Function32 {
        consts: ConstPool32::default(),
        code: vec![
            Instr32::abc(Opcode32::AddInt, 2, 0, 1),
            Instr32::abc(Opcode32::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 2,
        positional_param_count: 2,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function32::default()
    };
    let module = Arc::new(Module32 {
        functions: vec![callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
    });
    let state = Arc::new(Mutex::new(RuntimeModuleState32::new(HeapStore::new(), Vec::new())));
    let callable = RuntimeCallable32::with_state(Arc::clone(&module), 0, Arc::new(Vec::new()), Arc::clone(&state));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = call_runtime_callable32_test(&callable, &[RuntimeVal::Int(40), RuntimeVal::Int(2)], &mut ctx)
        .expect("call runtime callable");

    assert_eq!(result, vec![RuntimeVal::Int(42)]);
    let state = state.lock().expect("callable state");
    assert_eq!(
        state.stack_top(),
        0,
        "direct runtime callable calls must not leave their callee frame active"
    );
}

#[test]
fn execute_source32_runs_public_source_entry_on_new_vm() {
    let result = execute_source32(
        r#"
        let data = {"a": 40, "b": 2};
        let y = match data {
            {"a": a, ..rest} => a + rest.b,
        };
        return y;
        "#,
    )
    .expect("execute source");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_module32_context_native_can_use_vm_context() {
    fn add_seed(args: NativeArgs32<'_>, runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let [RuntimeVal::Int(delta)] = args.as_slice() else {
            bail!("add_seed expects one int");
        };
        let delta = *delta;
        let ctx = runtime.ctx_mut().ok_or_else(|| anyhow!("missing VmContext"))?;
        let Some(seed_export) = ctx.get_runtime_global("seed") else {
            bail!("seed must be a runtime global");
        };
        let RuntimeVal::Int(seed) = seed_export.value() else {
            bail!("seed must be an int runtime global");
        };
        let value = *seed + delta;
        ctx.define_runtime_value("seen", RuntimeVal::Int(value), HeapStore::new());
        Ok(RuntimeVal::Int(value))
    }

    let module = Compiler32::compile_source_module_with_natives(
        "return add_seed(2);",
        vec![NativeEntry32 {
            name: "add_seed".to_string(),
            arity: 1,
            function: NativeFunction32::Context(add_seed),
        }],
    )
    .expect("compile module");
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    ctx.define_runtime_value("seed", RuntimeVal::Int(40), HeapStore::new());

    let result = execute_module32_with_globals_and_ctx(&module, Vec::new(), &mut ctx).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        ctx.get_runtime_global("seen").map(|export| export.value()),
        Some(&RuntimeVal::Int(42))
    ));
}

#[test]
fn execute_program32_with_ctx_reads_external_slots_without_syncing_back_to_context() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        total := seed + 2;
        seed = total + 1;
        return seed;
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    ctx.define_runtime_value("seed", RuntimeVal::Int(39), HeapStore::new());

    let result = execute_program32_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        ctx.get_runtime_global("seed").map(|export| export.value()),
        Some(&RuntimeVal::Int(39))
    ));
    assert!(ctx.get_runtime_global("total").is_none());
}

#[test]
fn program_execute32_with_ctx_uses_new_vm_context_path() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        let value = seed + 2;
        return value;
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    ctx.define_runtime_value("seed", RuntimeVal::Int(40), HeapStore::new());

    let result = program.execute32_with_ctx(&mut ctx).expect("execute32");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_program32_imports_core_bit_builtins_as_runtime_native32() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        return (6 & 3) + (4 | 1) + (~0);
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program32_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(6)]);
}

#[test]
fn execute_program32_imports_core_object_builtins_as_runtime_native32() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        let boxed = __lk_make_struct("Box", {"x": 1});
        let updated = __lk_set_field(boxed, "y", 2);
        return __lk_merge_fields(updated, {"z": 3});
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program32_with_ctx(&program, &mut ctx).expect("execute");
    let map = result.first_return_map().expect("result map");

    assert_eq!(
        map.get(&RuntimeMapKey::String(Arc::from("x"))),
        Some(RuntimeVal::Int(1))
    );
    assert_eq!(
        map.get(&RuntimeMapKey::String(Arc::from("y"))),
        Some(RuntimeVal::Int(2))
    );
    assert_eq!(
        map.get(&RuntimeMapKey::String(Arc::from("z"))),
        Some(RuntimeVal::Int(3))
    );
}

#[test]
fn execute_program32_method_helper_uses_list_handle_positional_args() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        let parts = "crimson-long-name,emerald-long-name".split(",");
        return parts.join("|");
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program32_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.display_first_return(), "crimson-long-name|emerald-long-name");
}

#[test]
fn program_execute32_installs_core_method_helper_by_default() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        let parts = "red,blue".split(",");
        return parts.join("|");
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");

    let result = program.execute32().expect("execute");

    assert_eq!(result.display_first_return(), "red|blue");
}

#[test]
fn execute_program32_imports_typeof_as_runtime_native32() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        return typeof(__lk_make_struct("Box", {"x": 1}));
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program32_with_ctx(&program, &mut ctx).expect("execute");

    assert!(matches!(result.first_return(), RuntimeVal::ShortStr(value) if value.as_str() == "Object"));
}
