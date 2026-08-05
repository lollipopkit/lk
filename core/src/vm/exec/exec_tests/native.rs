use super::*;
use crate::util::fast_map::fast_hash_map_from_iter;
use crate::vm::ProgramExec;
use crate::vm::analysis::PerfGlobalFact;
/// A native is called through the same `Call` opcode as anything else, and the
/// argument window is cleared afterwards.
///
/// Written against an installed native rather than an inline `NativeEntry`: the
/// inline table is a mechanism no binary reaches (see
/// [`super::execute_source_with_natives`]), so this used to prove the property
/// for a path that does not ship.
#[test]
fn execute_module_calls_native_function_with_same_call_opcode() {
    fn native_add(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let [RuntimeVal::Int(lhs), RuntimeVal::Int(rhs)] = args.as_slice() else {
            bail!("native_add expects two ints");
        };
        Ok(RuntimeVal::Int(lhs + rhs))
    }

    let result = super::execute_source_with_natives(
        "return native_add(13, 29);",
        &[("native_add", NativeFunction::Plain(native_add), 2)],
    )
    .expect("execute source");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    // The call window is cleared: the arguments do not outlive the call, which
    // is what keeps a native's arguments from pinning heap values (see
    // `clear_call_window_temps`).
    // The same two slots the hand-built module asserted on: the argument window
    // is cleared after the call, so `13` and `29` do not outlive it. Slot 0
    // holds the callable the global was read into.
    assert_eq!(result.state.stack[1], RuntimeVal::Nil);
    assert_eq!(result.state.stack[2], RuntimeVal::Nil);
}

/// A native that allocates a value nobody keeps: the collector takes it.
///
/// Runs the program a user would write, with the collector set to run after
/// every allocation — the hand-built module got that by seeding its own
/// `HeapStore`, which is also why it could not reach an installed native.
#[test]
fn execute_module_collects_after_native_heap_allocation() {
    fn native_alloc_dead(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("dead-native-allocation")));
        Ok(RuntimeVal::Nil)
    }

    // Five hundred allocations nobody keeps, collected as they go.
    //
    // The old form asserted an *empty* heap, which a hand-built module can have
    // and a real program cannot — a program's heap holds its globals and the
    // callable this native was read from. What still separates "collected" from
    // "kept" is that the heap stays *bounded*: without the collector it would
    // carry all five hundred strings.
    let result = super::execute_source_with_natives_and_gc(
        "let i = 0;\nwhile i < 500 { native_alloc_dead(); i = i + 1; }\nreturn i;",
        &[("native_alloc_dead", NativeFunction::Plain(native_alloc_dead), 0)],
        Some(1),
    )
    .expect("execute source");

    assert_eq!(result.returns, vec![RuntimeVal::Int(500)]);
    assert!(
        result.state.heap.len() < 100,
        "five hundred unreferenced allocations were not collected: heap holds {}",
        result.state.heap.len()
    );
    assert!(!result.state.heap.should_collect());
}

#[test]
fn execute_module_calls_full_state_native_with_named_args() {
    fn full_state_clamp(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    // Named arguments to a native reached through a global, which is how every
    // stdlib native with named parameters is called. The hand-built module
    // spelled the same call in `CallNamed` operands against an inline table no
    // binary fills.
    let result = super::execute_source_with_natives(
        "return full_state_clamp(52, min: 40, max: 50);",
        &[(
            "full_state_clamp",
            NativeFunction::FullState(full_state_clamp),
            crate::vm::NativeEntry::VARIADIC,
        )],
    )
    .expect("execute source");

    assert_eq!(result.returns, vec![RuntimeVal::Int(50)]);
}

#[test]
fn execute_module_calls_runtime_callable_from_heap() {
    let callee = Function {
        consts: ConstPool {
            ints: vec![40],
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
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let callee_module = Arc::new(Module::single(callee));
    let callable = RuntimeCallable::with_state(
        Arc::clone(&callee_module),
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(RuntimeModuleState::new(HeapStore::new(), Vec::new()))),
    );

    let entry = Function {
        consts: ConstPool {
            ints: vec![2],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::GetGlobal, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
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
    let caller_module = Module {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "f".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result =
        execute_module_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx).expect("call runtime");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.stack[1], RuntimeVal::Nil);
}

#[test]
fn direct_runtime_closure_call_restores_state_after_arg_error() {
    let callee = Function {
        code: vec![Instr::abc(Opcode::Return, 0, 1, 0)],
        register_count: 2,
        param_count: 1,
        positional_param_count: 1,
        param_names: vec!["value".into()],
        ..Function::default()
    };
    let module = Module::single(callee);
    let mut state = RuntimeModuleState {
        stack_top: 1,
        ..Default::default()
    };
    state.stack.resize(1, RuntimeVal::Nil);
    state.stack[0] = RuntimeVal::Int(99);
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::Closure {
        function_index: 0,
        captures: Arc::new(Vec::new()),
    })));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err =
        call_runtime_value_runtime(callable, &[], &mut state, Some(&module), Some(&mut ctx)).expect_err("arity error");

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
    let callee = Function {
        code: vec![Instr::abc(Opcode::Return, 0, 1, 0)],
        register_count: 2,
        param_count: 1,
        positional_param_count: 0,
        param_names: vec!["value".into()],
        ..Function::default()
    };
    let module = Module::single(callee);
    let mut state = RuntimeModuleState {
        stack_top: 1,
        ..Default::default()
    };
    state.stack.resize(1, RuntimeVal::Nil);
    state.stack[0] = RuntimeVal::Int(77);
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::Closure {
        function_index: 0,
        captures: Arc::new(Vec::new()),
    })));
    let named = state.heap.alloc(HeapValue::String("not-a-map".into()));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err =
        call_runtime_value_runtime_named_map(callable, &[], Some(named), &mut state, Some(&module), Some(&mut ctx))
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
    fn full_state_named(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    let mut state = RuntimeModuleState::default();
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
        name: Arc::<str>::from("full_state_named"),
        arity: 1,
        function: NativeFunction::FullState(full_state_named),
    })));
    let named = state
        .heap
        .alloc(HeapValue::Map(TypedMap::StringInt(fast_hash_map_from_iter([(
            Arc::<str>::from("increment"),
            37,
        )]))));

    let mut ctx = VmContext::new_without_core_vm_builtins();
    let result = call_runtime_value_runtime_named_map(
        callable,
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
    fn native_alloc_live(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("native-dead")));
        let live = runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("native-live")));
        Ok(RuntimeVal::Obj(live))
    }

    let mut state = RuntimeModuleState::default();
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
        name: Arc::<str>::from("native_alloc_live"),
        arity: 0,
        function: NativeFunction::Plain(native_alloc_live),
    })));
    state.heap.set_gc_threshold(1);
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result =
        call_runtime_value_runtime(callable, &[], &mut state, None, Some(&mut ctx)).expect("direct runtime native");

    let RuntimeVal::Obj(live) = result else {
        panic!("native should return live heap object");
    };
    assert!(matches!(
        state.heap.get(live),
        Some(HeapValue::String(value)) if value.as_ref() == "native-live"
    ));
    // Handle 1: the string the native allocated before failing. Handle 0 is the
    // callable itself, which the global keeps alive.
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
    fn native_alloc_then_error(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("native-error-dead")));
        bail!("native failed after allocation");
    }

    let mut state = RuntimeModuleState::default();
    let callable = RuntimeVal::Obj(state.heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
        name: Arc::<str>::from("native_alloc_then_error"),
        arity: 0,
        function: NativeFunction::Plain(native_alloc_then_error),
    })));
    state.heap.set_gc_threshold(1);
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_value_runtime(callable, &[], &mut state, None, Some(&mut ctx))
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
fn execute_module_uses_global_slot_fact_for_get_and_set() {
    let mut entry = Function {
        consts: ConstPool {
            ints: vec![42],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::GetGlobal, 0, 0),
            Instr::abx(Opcode::LoadInt, 1, 0),
            Instr::abx(Opcode::SetGlobal, 1, 0),
            Instr::abx(Opcode::GetGlobal, 2, 0),
            Instr::abc(Opcode::Return, 2, 1, 0),
        ],
        register_count: 3,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
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
    let module = Module {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![
            GlobalSlot { name: "unused".into() },
            GlobalSlot { name: "answer".into() },
        ],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };

    let result =
        execute_module_with_globals(&module, vec![RuntimeVal::Int(0), RuntimeVal::Int(40)]).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.globals, vec![RuntimeVal::Int(0), RuntimeVal::Int(42)]);
}

#[test]
fn execute_module_set_global_move_fact_consumes_source_register() {
    let mut entry = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("stored-global-value"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abx(Opcode::SetGlobal, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    entry.performance.set_global_fact(
        1,
        PerfGlobalFact {
            slot: 0,
            move_source: true,
        },
    );
    let module = Module {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "stored".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };

    let result = execute_module_with_globals(&module, vec![RuntimeVal::Nil]).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Nil]);
    assert!(matches!(result.state.globals[0], RuntimeVal::Obj(_)));
}

#[test]
fn execute_module_set_global_without_move_fact_clones_source_register() {
    let entry = Function {
        consts: ConstPool {
            heap_values: vec![ConstHeapValue::LongString(Arc::<str>::from("stored-global-value"))],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadHeapConst, 0, 0),
            Instr::abx(Opcode::SetGlobal, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "stored".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };

    let result = execute_module_with_globals(&module, vec![RuntimeVal::Nil]).expect("execute module");

    assert!(matches!(result.returns[0], RuntimeVal::Obj(_)));
    assert!(matches!(result.state.globals[0], RuntimeVal::Obj(_)));
}

#[test]
fn execute_module_falls_back_to_instr_global_slot_without_fact() {
    let entry = Function {
        consts: ConstPool::default(),
        code: vec![Instr::abx(Opcode::GetGlobal, 0, 0), Instr::abc(Opcode::Return, 0, 1, 0)],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let module = Module {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "answer".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };

    let result = execute_module_with_globals(&module, vec![RuntimeVal::Int(42)]).expect("execute module");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert_eq!(result.state.inline_caches.global(0), Some(0));
}

#[test]
fn execute_caller_handler_catches_raise_from_runtime_callable() {
    let callee = Function {
        consts: ConstPool {
            strings: vec!["boom".into()],
            ..ConstPool::default()
        },
        code: vec![Instr::abx(Opcode::Raise, 0, 0)],
        register_count: 1,
        ..Function::default()
    };
    let callee_module = Arc::new(Module {
        functions: vec![callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    });
    let callable = RuntimeCallable::with_state(
        callee_module,
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(RuntimeModuleState::new(HeapStore::new(), Vec::new()))),
    );
    let entry = Function {
        code: vec![
            Instr::as_bx(Opcode::TryBegin, 0, 3),
            Instr::abx(Opcode::GetGlobal, 0, 0),
            Instr::abc(Opcode::Call, 0, 0, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
            Instr::abc(Opcode::Return, 0, 1, 0),
        ],
        register_count: 1,
        ..Function::default()
    };
    let caller_module = Module {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "f".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx)
        .expect("caller handler catches runtime raise");
    // A message-only raise binds the message *string* — the same contract as a
    // raise inside the frame or from a plain callee.
    assert_eq!(
        result.returns.first().expect("return"),
        &RuntimeVal::ShortStr(crate::val::ShortStr::new("boom").expect("short"))
    );
}

#[test]
fn execute_module_calls_runtime_callable_with_named_args() {
    let callee = Function {
        code: vec![Instr::abc(Opcode::AddInt, 2, 0, 1), Instr::abc(Opcode::Return, 2, 1, 0)],
        register_count: 3,
        param_count: 2,
        positional_param_count: 1,
        param_names: vec![Arc::<str>::from("x"), Arc::<str>::from("y")],
        capture_count: 0,
        ..Function::default()
    };
    let callee_module = Arc::new(Module {
        functions: vec![callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    });
    let callable = RuntimeCallable::with_state(
        Arc::clone(&callee_module),
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(RuntimeModuleState::new(HeapStore::new(), Vec::new()))),
    );

    let entry = Function {
        consts: ConstPool {
            ints: vec![40, 2],
            strings: vec!["y".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::GetGlobal, 0, 0),
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
    let caller_module = Module {
        functions: vec![entry],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "f".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    };
    let mut heap = HeapStore::new();
    let global = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::Runtime(Arc::new(callable)))));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = execute_module_with_globals_heap_and_ctx(&caller_module, vec![global], heap, &mut ctx)
        .expect("call runtime named");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn runtime_callable_error_keeps_shared_module_state() {
    let callee = Function {
        consts: ConstPool {
            ints: vec![41],
            strings: vec!["boom".to_string()],
            ..ConstPool::default()
        },
        code: vec![
            Instr::abx(Opcode::LoadInt, 0, 0),
            Instr::abx(Opcode::SetGlobal, 0, 0),
            Instr::abx(Opcode::Raise, 0, 0),
        ],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let callee_module = Arc::new(Module {
        functions: vec![callee],
        natives: Vec::new(),
        globals: vec![GlobalSlot { name: "counter".into() }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    });
    let callable = RuntimeCallable::with_state(
        Arc::clone(&callee_module),
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(RuntimeModuleState::new(
            HeapStore::new(),
            vec![RuntimeVal::Int(1)],
        ))),
    );
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_callable_test(&callable, &[], &mut ctx).expect_err("call should raise");

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
fn runtime_callable_native_error_collects_pending_heap_allocations() {
    fn native_alloc_then_error(_args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        runtime
            .heap_mut()
            .alloc(HeapValue::String(Arc::<str>::from("runtime-callable-native-dead")));
        bail!("runtime native failed after allocation");
    }

    let callee = Function {
        code: vec![Instr::abx(Opcode::GetGlobal, 0, 0), Instr::abc(Opcode::Call, 0, 0, 0)],
        register_count: 1,
        param_count: 0,
        positional_param_count: 0,
        param_names: Vec::new(),
        capture_count: 0,
        ..Function::default()
    };
    let callee_module = Arc::new(Module {
        functions: vec![callee],
        natives: Vec::new(),
        globals: vec![crate::vm::GlobalSlot {
            name: "native_alloc_then_error".into(),
        }],
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    });
    // The native lives in the callable's *own* state, as a global holding a
    // `RuntimeNative` — the shape a loaded module has. An inline `NativeEntry`
    // plus `LoadNative` is a mechanism nothing but these tests builds.
    let mut heap = HeapStore::new();
    let native = RuntimeVal::Obj(heap.alloc(HeapValue::Callable(CallableValue::RuntimeNative {
        name: Arc::<str>::from("native_alloc_then_error"),
        arity: 0,
        function: NativeFunction::Plain(native_alloc_then_error),
    })));
    let mut state = RuntimeModuleState::new(heap, vec![native]);
    state.heap.set_gc_threshold(1);
    let callable = RuntimeCallable::with_state(
        Arc::clone(&callee_module),
        0,
        Arc::new(Vec::new()),
        Arc::new(Mutex::new(state)),
    );
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_callable_test(&callable, &[], &mut ctx).expect_err("native should fail");

    assert!(err.to_string().contains("runtime native failed after allocation"));
    let state = callable.state.lock().expect("callable state");
    assert!(state.heap.get(HeapRef::new(1)).is_none());
    assert!(!state.heap.should_collect());
    assert_eq!(state.stack_top(), 0);
}

#[test]
fn direct_runtime_callable_restores_shared_state_stack_top() {
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
    let module = Arc::new(Module {
        functions: vec![callee],
        natives: Vec::new(),
        globals: Vec::new(),
        entry: 0,
        type_info: Default::default(),
        type_scope: Default::default(),
    });
    let state = Arc::new(Mutex::new(RuntimeModuleState::new(HeapStore::new(), Vec::new())));
    let callable = RuntimeCallable::with_state(Arc::clone(&module), 0, Arc::new(Vec::new()), Arc::clone(&state));
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let result = call_runtime_callable_test(&callable, &[RuntimeVal::Int(40), RuntimeVal::Int(2)], &mut ctx)
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
fn execute_source_runs_public_source_entry_on_new_vm() {
    let result = execute_source(
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

/// A map's own methods win over a key of the same name — for every method,
/// not just the one with its own opcode.
///
/// docs/semantics.md adjudicates "方法优先", and that was true of `len` alone:
/// the compiler emits a dedicated opcode for it, so it never reached the
/// dispatcher, where the key lookup ran *first*. `{"keys": 5, "z": 1}.keys()`
/// answered `5` and `{"is_empty": 5}.is_empty()` answered `5` — which of the
/// two you got depended on an implementation detail of the compiler.
///
/// The shadowed key keeps an unambiguous spelling (`m["len"]`), and a callable
/// stored under a name no builtin uses is still called.
#[test]
fn a_map_method_is_not_shadowed_by_a_key_of_the_same_name() {
    let result = execute_source(
        r#"
        let a = {"keys": 5, "z": 1};
        let b = {"values": 5, "z": 1};
        let c = {"len": 5, "z": 1};
        let d = {"is_empty": 5};
        let e = {"f": |x| { return x + 1; }, "n": 3};
        return [a.keys().len(), b.values().len(), c.len(), d.is_empty(), c["len"], e.f(4), e.n];
        "#,
    )
    .expect("execute source");

    let [RuntimeVal::Obj(handle)] = result.returns.as_slice() else {
        panic!("expected one list return");
    };
    let HeapValue::List(TypedList::Mixed(values)) = result.state.heap.get(*handle).expect("result list") else {
        panic!("expected mixed list return");
    };
    assert_eq!(values[0], RuntimeVal::Int(2), "keys() is the method, not the key");
    assert_eq!(values[1], RuntimeVal::Int(2), "values() is the method, not the key");
    assert_eq!(values[2], RuntimeVal::Int(2), "len() already was");
    assert_eq!(values[3], RuntimeVal::Bool(false), "is_empty() is the method");
    assert_eq!(values[4], RuntimeVal::Int(5), "the shadowed key is still readable");
    assert_eq!(values[5], RuntimeVal::Int(5), "a stored callable is still called");
    assert_eq!(values[6], RuntimeVal::Int(3), "and a plain key still reads");
}

#[test]
fn execute_source_uses_builtin_set_constructor_methods_and_iteration() {
    let result = execute_source(
        r#"
        let s = Set([1, 2, 2]);
        let added = s.add(3);
        let duplicate = s.add(2);
        let removed = s.delete(1);
        let total = 0;
        for value in s {
            total += value;
        }
        return [s.len(), s.contains(2), added, duplicate, removed, 3 in s, total, typeof(s)];
        "#,
    )
    .expect("execute source");

    let [RuntimeVal::Obj(handle)] = result.returns.as_slice() else {
        panic!("expected one list return");
    };
    let HeapValue::List(TypedList::Mixed(values)) = result.state.heap.get(*handle).expect("set result list") else {
        panic!("expected mixed list return");
    };
    assert_eq!(values[0], RuntimeVal::Int(2));
    assert_eq!(values[1], RuntimeVal::Bool(true));
    assert_eq!(values[2], RuntimeVal::Bool(true));
    assert_eq!(values[3], RuntimeVal::Bool(false));
    assert_eq!(values[4], RuntimeVal::Bool(true));
    assert_eq!(values[5], RuntimeVal::Bool(true));
    assert_eq!(values[6], RuntimeVal::Int(5));
    assert_eq!(values[7], RuntimeVal::ShortStr(ShortStr::new("Set").expect("short")));
}

#[test]
fn execute_source_rejects_float_set_values() {
    let err = execute_source(
        r#"
        let s = Set();
        s.add(1.5);
        return s;
        "#,
    )
    .expect_err("float set value should fail");
    assert!(
        err.to_string().contains("Float cannot be a map key or set member"),
        "{err}"
    );
}

#[test]
fn execute_module_context_native_can_use_vm_context() {
    fn add_seed(args: NativeArgs<'_>, runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
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

    // Installed on the context and named as an external global: how a stdlib
    // native reaches a program. Compiling one *into* the module was the
    // `LoadNative` path, which no binary built.
    let program = crate::syntax::parse_program_source("return add_seed(2);", Default::default()).expect("parse");
    let mut ctx = crate::vm::VmContext::new_without_core_vm_builtins();
    ctx.install_runtime_builtin("add_seed", NativeFunction::Context(add_seed), 1);
    ctx.define_runtime_value("seed", RuntimeVal::Int(40), HeapStore::new());

    // The program path, because that is what seeds a module's globals from the
    // context (`seed_module_globals`); `execute_module_with_globals_and_ctx`
    // takes the values from its caller and this one has none to give.
    let result = crate::vm::execute_program_with_ctx(&program, &mut ctx).expect("execute program");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        ctx.get_runtime_global("seen").map(|export| export.value()),
        Some(&RuntimeVal::Int(42))
    ));
}

#[test]
fn execute_program_with_ctx_reads_external_slots_without_syncing_back_to_context() {
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

    let result = execute_program_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        ctx.get_runtime_global("seed").map(|export| export.value()),
        Some(&RuntimeVal::Int(39))
    ));
    assert!(ctx.get_runtime_global("total").is_none());
}

#[test]
fn program_execute_with_ctx_uses_new_vm_context_path() {
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

    let result = program.execute_with_ctx(&mut ctx).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn execute_program_imports_core_bit_builtins_as_runtime_native() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        return (6 & 3) + (4 | 1) + (~0);
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(6)]);
}

#[test]
fn execute_program_imports_core_object_builtins_as_runtime_native() {
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

    let result = execute_program_with_ctx(&program, &mut ctx).expect("execute");
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
fn execute_program_method_helper_uses_list_handle_positional_args() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        let parts = "crimson-long-name,emerald-long-name".split(",");
        return parts.join("|");
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.display_first_return(), "crimson-long-name|emerald-long-name");
}

#[test]
fn program_execute_installs_core_method_helper_by_default() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        let parts = "red,blue".split(",");
        return parts.join("|");
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");

    let result = program.execute().expect("execute");

    assert_eq!(result.display_first_return(), "red|blue");
}

/// `typeof` on a struct instance names the struct.
///
/// It answered `Object` — the heap representation, which is not a type the
/// language has, and the same answer for every struct in the program. That made
/// `typeof` useless on exactly the values a program most wants to ask about.
/// `HeapValue::type_name` is the carrier of that rule and had the hole itself.
#[test]
fn execute_program_imports_typeof_as_runtime_native() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        return typeof(__lk_make_struct("Box", {"x": 1}));
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program_with_ctx(&program, &mut ctx).expect("execute");

    assert!(matches!(result.first_return(), RuntimeVal::ShortStr(value) if value.as_str() == "Box"));
}

/// A `Set` is a map's key set, so it rejects what a map rejects. It used to
/// take a list as a member and compare it by *handle*, so the member could
/// never be found again and two equal lists both went in.
#[test]
fn execute_source_rejects_container_set_members_like_map_keys() {
    for source in [
        "let s = Set(); s.add([1, 2]); return s;",
        "let s = Set([[1, 2]]); return s;",
        "let m = {}; m.set([1, 2], 3); return m;",
    ] {
        let err = execute_source(source).expect_err("a list is not a key");
        assert!(
            err.to_string().contains("List cannot be a map key or set member"),
            "{source} → {err}"
        );
    }
}
