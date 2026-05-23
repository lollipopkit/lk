use super::*;
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
    let callable = RuntimeCallable32::new(Arc::clone(&callee_module), 0, Vec::new(), HeapStore::new(), Vec::new());

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
    let callable = RuntimeCallable32::new(callee_module, 0, Vec::new(), HeapStore::new(), Vec::new());
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
    let callable = RuntimeCallable32::new(Arc::clone(&callee_module), 0, Vec::new(), HeapStore::new(), Vec::new());

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
    let callable = RuntimeCallable32::new(
        Arc::clone(&callee_module),
        0,
        Vec::new(),
        HeapStore::new(),
        vec![RuntimeVal::Int(1)],
    );
    let mut ctx = VmContext::new_without_core_vm_builtins();

    let err = call_runtime_callable32_raw(&callable, &[], &mut ctx).expect_err("call should raise");

    assert!(err.to_string().contains("boom"));
    let state = callable.state.lock().expect("callable state");
    assert_eq!(state.globals, vec![RuntimeVal::Int(41)]);
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
        let RuntimeVal::Int(seed) = seed_export.value else {
            bail!("seed must be an int runtime global");
        };
        let value = seed + delta;
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
        ctx.get_runtime_global("seen").map(|export| &export.value),
        Some(RuntimeVal::Int(42))
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

    let result = execute_program32_raw_with_ctx(&program, &mut ctx).expect("execute");

    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
    assert!(matches!(
        ctx.get_runtime_global("seed").map(|export| &export.value),
        Some(RuntimeVal::Int(39))
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

    let result = execute_program32_raw_with_ctx(&program, &mut ctx).expect("execute");

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

    let result = execute_program32_raw_with_ctx(&program, &mut ctx).expect("execute");
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
fn execute_program32_imports_typeof_as_runtime_native32() {
    let tokens = crate::token::Tokenizer::tokenize(
        r#"
        return typeof(__lk_make_struct("Box", {"x": 1}));
        "#,
    )
    .expect("tokenize");
    let program = crate::stmt::StmtParser::new(&tokens).parse_program().expect("parse");
    let mut ctx = crate::vm::VmContext::new();

    let result = execute_program32_raw_with_ctx(&program, &mut ctx).expect("execute");

    assert!(matches!(result.first_return(), RuntimeVal::ShortStr(value) if value.as_str() == "Object"));
}
