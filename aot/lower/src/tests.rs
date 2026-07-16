use super::*;
use lk_core::vm::{ConstPoolData, FunctionData, MODULE_ARTIFACT_VERSION, ModuleData};

fn artifact(consts: ConstPoolData, code: Vec<u32>, register_count: u16) -> ModuleArtifact {
    ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![FunctionData {
                consts,
                code,
                performance: Default::default(),
                register_count,
                param_count: 0,
                positional_param_count: 0,
                param_names: Vec::new(),
                capture_count: 0,
                debug_name: None,
            }],
        },
    }
}

fn ints(v: Vec<i64>) -> ConstPoolData {
    ConstPoolData {
        ints: v,
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: Vec::new(),
    }
}

fn floats(v: Vec<f64>) -> ConstPoolData {
    ConstPoolData {
        ints: Vec::new(),
        floats: v,
        strings: Vec::new(),
        heap_values: Vec::new(),
    }
}

fn func(consts: ConstPoolData, code: Vec<u32>, register_count: u16, param_count: u16) -> FunctionData {
    FunctionData {
        consts,
        code,
        performance: Default::default(),
        register_count,
        param_count,
        positional_param_count: param_count,
        param_names: Vec::new(),
        capture_count: 0,
        debug_name: None,
    }
}

/// `let inc = |x| x + 1; return inc(41);` — a zero-capture closure stored in
/// a module global (entry-prefix single assignment), read back and called
/// indirectly: devirtualizes to a direct `CallFn`.
#[test]
fn lowers_zero_capture_lambda_global_call() {
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: vec!["inc".to_string()],
            functions: vec![
                func(
                    ints(vec![41]),
                    vec![
                        Instr::abc(Opcode::MakeClosure, 0, 1, 0).raw(),
                        Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                        Instr::abx(Opcode::GetGlobal, 1, 0).raw(),
                        Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                        Instr::abc(Opcode::Call, 1, 0, 1).raw(),
                        Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                    ],
                    4,
                    0,
                ),
                func(
                    ints(vec![1]),
                    vec![
                        Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                        Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),
                        Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
                    ],
                    3,
                    1,
                ),
            ],
        },
    };
    let mir = lower(&art).expect("zero-capture lambda lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert_eq!(mir.functions.len(), 2, "lambda body must be reachable/emitted");
}

/// A register-local zero-capture lambda (no global storage) calls directly
/// through the tracked `MakeClosure` ref.
#[test]
fn lowers_local_lambda_call() {
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                func(
                    ints(vec![20]),
                    vec![
                        Instr::abc(Opcode::MakeClosure, 1, 1, 0).raw(),
                        Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                        Instr::abc(Opcode::Call, 1, 0, 1).raw(),
                        Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                    ],
                    4,
                    0,
                ),
                func(
                    ints(vec![2]),
                    vec![
                        Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                        Instr::abc(Opcode::MulInt, 2, 0, 1).raw(),
                        Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
                    ],
                    3,
                    1,
                ),
            ],
        },
    };
    let mir = lower(&art).expect("local lambda lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert_eq!(mir.functions.len(), 2);
}

/// A capturing closure (`capture_count == 1`) rejects the module.
#[test]
fn rejects_capturing_closure() {
    let mut lambda = func(ints(vec![]), vec![Instr::abc(Opcode::Return1, 0, 0, 0).raw()], 2, 1);
    lambda.capture_count = 1;
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: Vec::new(),
            functions: vec![
                func(
                    ints(vec![]),
                    vec![
                        Instr::abc(Opcode::MakeClosure, 0, 1, 0).raw(),
                        Instr::abc(Opcode::Return1, 0, 0, 0).raw(),
                    ],
                    2,
                    0,
                ),
                lambda,
            ],
        },
    };
    assert!(lower(&art).is_err(), "capturing closure must reject");
}

/// A lambda global written twice is not single-assignment: readers could
/// observe either closure, so the module rejects loudly.
#[test]
fn rejects_reassigned_lambda_global() {
    let lambda_body = |k: i64| {
        func(
            ints(vec![k]),
            vec![
                Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                Instr::abc(Opcode::MulInt, 2, 0, 1).raw(),
                Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            ],
            3,
            1,
        )
    };
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: vec!["f".to_string()],
            functions: vec![
                func(
                    ints(vec![5]),
                    vec![
                        Instr::abc(Opcode::MakeClosure, 0, 1, 0).raw(),
                        Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                        Instr::abc(Opcode::MakeClosure, 0, 2, 0).raw(),
                        Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                        Instr::abx(Opcode::GetGlobal, 1, 0).raw(),
                        Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                        Instr::abc(Opcode::Call, 1, 0, 1).raw(),
                        Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                    ],
                    4,
                    0,
                ),
                lambda_body(1),
                lambda_body(2),
            ],
        },
    };
    assert!(lower(&art).is_err(), "reassigned lambda global must reject");
}

/// `fn add(x,y){ return x+y } return add(3,4)` — a two-function module with a
/// register-window direct call (`CallDirect a=1 b=1 c=2`, args at r2/r3).
#[test]
fn lowers_direct_call() {
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: vec!["add".to_string()],
            functions: vec![
                func(
                    ints(vec![3, 4]),
                    vec![
                        Instr::abx(Opcode::LoadFunction, 0, 1).raw(),
                        Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                        Instr::abx(Opcode::LoadInt, 2, 0).raw(),
                        Instr::abx(Opcode::LoadInt, 3, 1).raw(),
                        Instr::abc(Opcode::CallDirect, 1, 1, 2).raw(),
                        Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                    ],
                    4,
                    0,
                ),
                func(
                    ints(vec![]),
                    vec![
                        Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),
                        Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
                    ],
                    3,
                    2,
                ),
            ],
        },
    };
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert_eq!(mir.functions.len(), 2);
    // fn 1 is (i64, i64) -> i64.
    assert_eq!(mir.functions[1].params.len(), 2);
    assert_eq!(mir.functions[1].ret, Ty::I64);
    // The callee body adds its two params and returns the sum.
}

/// `["a","b","c"].join("-")` — a constant `List<str>` materializes a str-list
/// handle (new + str_push) and `ListJoin` lowers to `str_join`.
#[test]
fn lowers_str_list_join() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: Vec::new(),
        floats: Vec::new(),
        strings: vec!["-".to_string()],
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::ShortStr("a".to_string()),
            ConstRuntimeValueData::ShortStr("b".to_string()),
            ConstRuntimeValueData::ShortStr("c".to_string()),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = ["a","b","c"]
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
            Instr::abx(Opcode::LoadString, 1, 0).raw(),    // r1 = "-"
            Instr::abc(Opcode::ListJoin, 2, 0, 1).raw(),   // r2 = xs.join("-")
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("str list + join lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `2 in xs` (`Contains a=dst b=needle c=haystack`) lowers to the list membership
/// helper, narrowed from the runtime's `0/1` to an `i1`.
#[test]
fn lowers_in_operator() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![2],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::Int(1),
            ConstRuntimeValueData::Int(2),
            ConstRuntimeValueData::Int(3),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = [1,2,3]
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
            Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 2 (needle)
            Instr::abc(Opcode::Contains, 2, 1, 0).raw(),   // r2 = (2 in xs)
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("in-operator lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// A defined-but-never-called function (here with an unsupported out-of-range
/// constant) is dead for AOT: it is skipped, so it does not fail the module, and
/// only the entry is emitted.
#[test]
fn dead_function_is_skipped() {
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: vec!["dead".to_string()],
            functions: vec![
                func(
                    ints(vec![42]),
                    vec![
                        Instr::abx(Opcode::LoadFunction, 0, 1).raw(), // registers `dead`, never calls it
                        Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                        Instr::abx(Opcode::LoadInt, 1, 0).raw(),
                        Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                    ],
                    2,
                    0,
                ),
                // `dead`: an out-of-range const would be `Unsupported`, but it is
                // never reached, so it must not fail the module.
                func(
                    ints(vec![]),
                    vec![
                        Instr::abx(Opcode::LoadInt, 0, 99).raw(),
                        Instr::abc(Opcode::Return1, 0, 0, 0).raw(),
                    ],
                    1,
                    0,
                ),
            ],
        },
    };
    let mir = lower(&art).expect("dead function does not block lowering");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert_eq!(mir.functions.len(), 1, "only the reachable entry is emitted");
    assert_eq!(mir.functions[0].id, FuncId(0));
}

/// `fn f(x){ return x } return f(4.0)` — the callee's parameter type is
/// monomorphized to `F64` from the (single, consistent) `f64` call site, so `f`
/// lowers as `double @lk_fn_1(double)`.
#[test]
fn monomorphizes_f64_parameter() {
    let art = ModuleArtifact {
        format: "lk.module".to_string(),
        version: MODULE_ARTIFACT_VERSION,
        imports: Vec::new(),
        module: ModuleData {
            entry: 0,
            globals: vec!["f".to_string()],
            functions: vec![
                func(
                    floats(vec![4.0]),
                    vec![
                        Instr::abx(Opcode::LoadFunction, 0, 1).raw(),
                        Instr::abx(Opcode::SetGlobal, 0, 0).raw(),
                        Instr::abx(Opcode::LoadFloat, 2, 0).raw(), // r2 = 4.0
                        Instr::abc(Opcode::CallDirect, 1, 1, 1).raw(), // r1 = f(r2)
                        Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
                    ],
                    3,
                    0,
                ),
                func(ints(vec![]), vec![Instr::abc(Opcode::Return1, 0, 0, 0).raw()], 1, 1),
            ],
        },
    };
    let mir = lower(&art).expect("lowers with f64 param");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert_eq!(mir.functions[1].params[0].1, Ty::F64);
    assert_eq!(mir.functions[1].ret, Ty::F64);
}

/// `let xs = [10,20,30]; return xs.len();` — a constant `List<i64>` materialized
/// into a growable `lkrt` handle, then `Len`.
#[test]
fn lowers_const_list_len() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: Vec::new(),
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::Int(10),
            ConstRuntimeValueData::Int(20),
            ConstRuntimeValueData::Int(30),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),
            Instr::abc(Opcode::Len, 1, 0, 0).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        2,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert_eq!(mir.functions[0].ret, Ty::I64);
}

/// `let xs=[10,20,30]; return xs[0];` — a provably in-range constant index on a
/// const-materialized list lowers to a runtime `lkrt_lklist_i64_at`.
#[test]
fn lowers_const_inbounds_index() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![0],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::Int(10),
            ConstRuntimeValueData::Int(20),
            ConstRuntimeValueData::Int(30),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),
            Instr::abx(Opcode::LoadInt, 2, 0).raw(), // index 0
            Instr::abc(Opcode::GetList, 1, 0, 2).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `let xs=[10]; xs.push(20); xs.push(30); return xs[2];` — in-place push grows
/// the tracked length so a later constant index stays provably in range.
#[test]
fn lowers_list_push_then_index() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![20, 30, 2],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![ConstRuntimeValueData::Int(10)])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = [10]
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
            Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 20
            Instr::abc(Opcode::ListPush, 0, 1, 0).raw(),   // xs.push(20)
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),       // r1 = 30
            Instr::abc(Opcode::ListPush, 0, 1, 0).raw(),   // xs.push(30)
            Instr::abx(Opcode::LoadInt, 1, 2).raw(),       // r1 = 2 (index)
            Instr::abc(Opcode::GetList, 1, 0, 1).raw(),    // xs[2]
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        2,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `let xs=[10,20,30]; xs[1]=99; return xs[1];` — a store lowers to the
/// bounds-checked `i64_set` helper, and the subsequent in-range read still folds
/// to a clean `at`.
#[test]
fn lowers_set_index() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![1, 99],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::Int(10),
            ConstRuntimeValueData::Int(20),
            ConstRuntimeValueData::Int(30),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = [10,20,30]
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
            Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 1 (index)
            Instr::abx(Opcode::LoadInt, 2, 1).raw(),       // r2 = 99 (value)
            Instr::abc(Opcode::SetIndex, 0, 1, 2).raw(),   // xs[1] = 99
            Instr::abx(Opcode::LoadInt, 2, 0).raw(),       // r2 = 1 (index)
            Instr::abc(Opcode::GetList, 1, 0, 2).raw(),    // xs[1]
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("lowers set + read");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    // The read after the store is provably in range → clean `at`, not a Maybe.
}

/// A dynamic index of an `f64` list produces a `MaybeF64`; consumed by a return
/// it renders the by-value `f64` get-pair and a present-branching print.
#[test]
fn dynamic_f64_index_lowers_to_maybe_f64() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![0, 1],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::Float(1.5),
            ConstRuntimeValueData::Float(2.5),
        ])],
    };
    // Build a non-constant index (r1 = 0; r1 = r1 + 1) so the access is dynamic.
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 2, 0).raw(), // r2 = [1.5,2.5]
            Instr::abc(Opcode::Move, 0, 2, 0).raw(),       // r0 = xs
            Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 0
            Instr::abc(Opcode::AddIntI, 1, 1, 1).raw(),    // r1 = r1 + 1 (dynamic)
            Instr::abc(Opcode::GetList, 1, 0, 1).raw(),    // r1 = xs[r1]
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("f64 dynamic index lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// The `str` analogue of the dynamic-index `Maybe` model: a non-constant index
/// into a `List<str>` lowers to `lkrt_lklist_str_get_pair` (`{ptr, i64}`), and
/// the returned `Maybe<str>` prints the element or nothing.
#[test]
fn dynamic_str_index_lowers_to_maybe_str() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![0, 1],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::ShortStr("foo".to_string()),
            ConstRuntimeValueData::ShortStr("bar".to_string()),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 2, 0).raw(), // r2 = ["foo","bar"]
            Instr::abc(Opcode::Move, 0, 2, 0).raw(),       // r0 = xs
            Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 0
            Instr::abc(Opcode::AddIntI, 1, 1, 1).raw(),    // r1 = r1 + 1 (dynamic)
            Instr::abc(Opcode::GetList, 1, 0, 1).raw(),    // r1 = xs[r1]
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("str dynamic index lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// Fused `acc += list[index]` (`AddListInt`): with a provably in-range constant
/// index the element folds to a clean `at`, then an integer add.
#[test]
fn lowers_add_list_int() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![5, 1],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::Int(10),
            ConstRuntimeValueData::Int(20),
            ConstRuntimeValueData::Int(30),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = [10,20,30]
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = xs
            Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 5 (acc)
            Instr::abx(Opcode::LoadInt, 2, 1).raw(),       // r2 = 1 (index)
            Instr::abc(Opcode::AddListInt, 1, 0, 2).raw(), // r1 += xs[1]
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("lowers AddListInt");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `let m = {"a": 7}; return m["a"];` — a constant `Map<str,i64>` materializes a
/// map handle (new + set), and `GetFieldK` lowers to the by-value Maybe lookup;
/// the constant key is interned as a single shared global.
#[test]
fn lowers_str_map_const_and_get() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: Vec::new(),
        floats: Vec::new(),
        strings: vec!["a".to_string()],
        heap_values: vec![ConstHeapValueData::Map(vec![(
            RuntimeMapKeyData::ShortStr("a".to_string()),
            ConstRuntimeValueData::Int(7),
        )])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = {"a":7}
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = m
            Instr::abc(Opcode::GetFieldK, 1, 0, 0).raw(),  // r1 = m["a"] (key strings[0])
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        2,
    );
    let mir = lower(&art).expect("map const + get lowers");
    assert_eq!(mir.globals, vec!["a".to_string()], "key interned once");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `if (m[k] == nil)` via `BrNotNil` on a map lookup: the `Maybe`'s present bit
/// drives the branch (`extractvalue … 1` → `icmp ne`).
#[test]
fn lowers_nil_branch_on_maybe() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![9, 1, 0],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::Map(vec![(
            RuntimeMapKeyData::Int(1),
            ConstRuntimeValueData::Int(10),
        )])],
    };
    // r0 = {1:10}; r1 = m[9] (Maybe, missing); if (r1 != nil) goto else(pc7) else then(pc5)
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = {1:10}
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = m
            Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 9 (key)
            Instr::abc(Opcode::GetIndex, 1, 0, 1).raw(),   // r1 = m[9] (Maybe)
            Instr::as_bx(Opcode::BrNotNil, 1, 2).raw(),    // sbx=2 -> jump to pc7 when not-nil
            Instr::abx(Opcode::LoadInt, 2, 1).raw(),       // pc5: r2 = 1 (then: is nil)
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
            Instr::abx(Opcode::LoadInt, 2, 2).raw(), // pc7: r2 = 0 (else)
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("nil-branch on maybe lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `if (x % 4 == 0) { return 1 } else { return 0 }` via the fused
/// `BrModNeZeroIntI4` divisibility branch (guarded modulo + compare-to-zero).
#[test]
fn lowers_fused_mod_zero_branch() {
    // pc0: r0 = 12
    // pc1: BrModNeZeroIntI4 r0 % 4 != 0, offset=2 (jump to else pc4 when != 0)
    // pc2: r1 = 1 ; pc3: return r1   (then: divisible)
    // pc4: r1 = 0 ; pc5: return r1   (else)
    let art = artifact(
        ints(vec![12, 1, 0]),
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),
            Instr::branch_i4(Opcode::BrModNeZeroIntI4, 0, 4, 2).raw(),
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            Instr::abx(Opcode::LoadInt, 1, 2).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        2,
    );
    let mir = lower(&art).expect("fused mod-zero branch lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `if (x == 3) { return 100 } else { return 0 }` via the fused `BrNeIntI4`
/// branch: the single-instruction compare-and-branch lowers like a `CondBr`.
#[test]
fn lowers_fused_ne_immediate_branch() {
    // pc0: r0 = 3
    // pc1: BrNeIntI4 r0 != 3, offset=2  (jump to else pc4 when !=)
    // pc2: r1 = 100  (then, r0 == 3)
    // pc3: return r1
    // pc4: r1 = 0    (else)
    // pc5: return r1
    let art = artifact(
        ints(vec![3, 100, 0]),
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),
            Instr::branch_i4(Opcode::BrNeIntI4, 0, 3, 2).raw(),
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
            Instr::abx(Opcode::LoadInt, 1, 2).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        2,
    );
    let mir = lower(&art).expect("fused ne-branch lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// A returned `f64` prints via `lkrt_f64_to_str` (Rust `to_string`, the VM's exact
/// float display) rather than `printf %g` — whose fixed precision diverges from
/// the VM's shortest round-trip (e.g. `1.0/7.0`).
#[test]
fn float_return_uses_display_helper() {
    let art = artifact(
        floats(vec![1.5, 2.5]),
        vec![
            Instr::abx(Opcode::LoadFloat, 0, 0).raw(),
            Instr::abx(Opcode::LoadFloat, 1, 1).raw(),
            Instr::abc(Opcode::AddFloat, 2, 0, 1).raw(),
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("float return lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `"n=${n}"` — numeric interpolation lowers `ConcatString` with an `I64` operand
/// display-converted via `str.from_i64`.
#[test]
fn lowers_concat_string_int_display() {
    let consts = ConstPoolData {
        ints: vec![5],
        floats: Vec::new(),
        strings: vec!["n=".to_string()],
        heap_values: Vec::new(),
    };
    // r0 = 5; r1 = "n="; ConcatString dst=2 b=1 c=0 → "n=" ++ display(5)
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),
            Instr::abx(Opcode::LoadString, 1, 0).raw(),
            Instr::abc(Opcode::ConcatString, 2, 1, 0).raw(),
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("ConcatString with int display lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    // The int suffix fuses into a single concat_i64 call — no intermediate
    // display string is materialized (or freed).
}

/// `"${a}-${b}"` — string interpolation of string vars lowers `ConcatN` to a
/// chain of `str_concat`.
#[test]
fn lowers_concat_n_strings() {
    let consts = ConstPoolData {
        ints: Vec::new(),
        floats: Vec::new(),
        strings: vec!["a".to_string(), "-".to_string(), "b".to_string()],
        heap_values: Vec::new(),
    };
    // r0="a", r1="-", r2="b"; ConcatN dst=3 start=0 count=3 → "a-b"
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadString, 0, 0).raw(),
            Instr::abx(Opcode::LoadString, 1, 1).raw(),
            Instr::abx(Opcode::LoadString, 2, 2).raw(),
            Instr::abc(Opcode::ConcatN, 3, 0, 3).raw(),
            Instr::abc(Opcode::Return1, 3, 0, 0).raw(),
        ],
        4,
    );
    let mir = lower(&art).expect("ConcatN of strings lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    // 3 elements → 2 chained concats.
}

/// `a + b` on two strings is concatenation (the generic `AddInt` opcode) →
/// `lkrt_str_concat`, yielding a `Str`.
#[test]
fn lowers_string_concat() {
    let consts = ConstPoolData {
        ints: Vec::new(),
        floats: Vec::new(),
        strings: vec!["foo".to_string(), "bar".to_string()],
        heap_values: Vec::new(),
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadString, 0, 0).raw(), // r0 = "foo"
            Instr::abx(Opcode::LoadString, 1, 1).raw(), // r1 = "bar"
            Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),  // r2 = r0 + r1
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("string concat lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    // The concat result is a Str, printed via %s on return.
}

/// `"hi" == "hi"` — string equality via the generic `CmpInt` opcode on two `Str`
/// operands lowers to `str_cmp` compared against 0.
#[test]
fn lowers_string_equality() {
    let consts = ConstPoolData {
        ints: Vec::new(),
        floats: Vec::new(),
        strings: vec!["hi".to_string()],
        heap_values: Vec::new(),
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadString, 0, 0).raw(), // r0 = "hi"
            Instr::abx(Opcode::LoadString, 1, 0).raw(), // r1 = "hi"
            Instr::abc(Opcode::CmpInt, 2, 0, 1).raw(),  // r2 = (r0 == r1)
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("string equality lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `!(x > 3)` — logical `Not` on a `Bool` lowers to `xor i1 …, true`.
#[test]
fn lowers_logical_not() {
    let art = artifact(
        ints(vec![5, 3]),
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),     // r0 = 5
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),     // r1 = 3
            Instr::abc(Opcode::CmpGtInt, 2, 0, 1).raw(), // r2 = (5 > 3)
            Instr::abc(Opcode::Not, 3, 2, 0).raw(),      // r3 = !r2
            Instr::abc(Opcode::Return1, 3, 0, 0).raw(),
        ],
        4,
    );
    let mir = lower(&art).expect("logical not lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `DivFloat` (and the other float ops) coerce an `I64` operand to `F64` (the VM
/// does this too, e.g. an `I64` parameter in `x / 2.0`): `10 / 2.0 => 5.0`.
#[test]
fn float_arith_coerces_int_operand() {
    let art = artifact(
        ConstPoolData {
            ints: vec![10],
            floats: vec![2.0],
            strings: Vec::new(),
            heap_values: Vec::new(),
        },
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),     // r0 = 10 (i64)
            Instr::abx(Opcode::LoadFloat, 1, 0).raw(),   // r1 = 2.0 (f64)
            Instr::abc(Opcode::DivFloat, 2, 0, 1).raw(), // r2 = r0 / r1
            Instr::abc(Opcode::Return1, 2, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("float div with int operand lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// An empty `{}` used with an int-index store is typed int-keyed by lookahead:
/// `let m = {}; m[5] = 50; return m[5];` lowers via the `i64_i64` map handle.
#[test]
fn empty_map_int_key_lookahead() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![5, 50],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::Map(Vec::new())], // {}
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = {}
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = m
            Instr::abx(Opcode::LoadInt, 1, 0).raw(),       // r1 = 5 (key)
            Instr::abx(Opcode::LoadInt, 2, 1).raw(),       // r2 = 50 (value)
            Instr::abc(Opcode::SetIndex, 0, 1, 2).raw(),   // m[5] = 50
            Instr::abx(Opcode::LoadInt, 2, 0).raw(),       // r2 = 5 (key)
            Instr::abc(Opcode::GetIndex, 1, 0, 2).raw(),   // r1 = m[5]
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("empty int-key map lowers via lookahead");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    // `str_i64` symbols appear in the prelude declarations, but no string-keyed
    // map is *called* here.
}

/// `let m = {1: 1.5}; return m[1];` — a constant int-keyed f64-valued map
/// materializes an `i64→f64` handle; `GetIndex` yields a `MaybeF64`.
#[test]
fn lowers_int_f64_map() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![1],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::Map(vec![(
            RuntimeMapKeyData::Int(1),
            ConstRuntimeValueData::Float(1.5),
        )])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),
            Instr::abx(Opcode::LoadInt, 2, 0).raw(),
            Instr::abc(Opcode::GetIndex, 1, 0, 2).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("int-f64 map lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `let m = {"a": 1.5}; return m["a"];` — a constant str-keyed f64-valued map
/// materializes an `str→f64` handle; `GetFieldK` yields a `MaybeF64`.
#[test]
fn lowers_str_f64_map() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: Vec::new(),
        floats: Vec::new(),
        strings: vec!["a".to_string()],
        heap_values: vec![ConstHeapValueData::Map(vec![(
            RuntimeMapKeyData::ShortStr("a".to_string()),
            ConstRuntimeValueData::Float(1.5),
        )])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),
            Instr::abc(Opcode::GetFieldK, 1, 0, 0).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        2,
    );
    let mir = lower(&art).expect("str-f64 map lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `let m = {1:10, 2:20}; return m[2];` — a constant int-keyed map materializes an
/// `i64→i64` handle (new + set), and `GetIndex` lowers to the by-value Maybe lookup.
#[test]
fn lowers_int_key_map() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![2],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::Map(vec![
            (RuntimeMapKeyData::Int(1), ConstRuntimeValueData::Int(10)),
            (RuntimeMapKeyData::Int(2), ConstRuntimeValueData::Int(20)),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(), // r1 = {1:10, 2:20}
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),       // r0 = m
            Instr::abx(Opcode::LoadInt, 2, 0).raw(),       // r2 = 2 (key)
            Instr::abc(Opcode::GetIndex, 1, 0, 2).raw(),   // r1 = m[2]
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("int-key map lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    // The literal builds through the lit protocol (VM-order mirror).
}

/// A returned string literal materializes an interned global and prints via the
/// entry's `%s` path.
#[test]
fn lowers_string_constant_return() {
    let consts = ConstPoolData {
        ints: Vec::new(),
        floats: Vec::new(),
        strings: vec!["hello".to_string()],
        heap_values: Vec::new(),
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadString, 0, 0).raw(), // r0 = "hello"
            Instr::abc(Opcode::Return1, 0, 0, 0).raw(),
        ],
        1,
    );
    let mir = lower(&art).expect("string constant lowers");
    assert_eq!(mir.globals, vec!["hello".to_string()]);
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// Identical string constants intern to a single shared global.
#[test]
fn interns_duplicate_strings() {
    let mut globals = vec![];
    let a = intern_global(&mut globals, "k");
    let b = intern_global(&mut globals, "k");
    let c = intern_global(&mut globals, "other");
    assert_eq!((a, b, c), (0, 0, 1));
    assert_eq!(globals, vec!["k".to_string(), "other".to_string()]);
}

/// A dead `LoadString` (common in loop setup) must not block lowering: the
/// register is left undefined and the surrounding integer code still lowers.
#[test]
fn dead_string_load_does_not_block_lowering() {
    let consts = ConstPoolData {
        ints: vec![42],
        floats: Vec::new(),
        strings: vec!["unused".to_string()],
        heap_values: Vec::new(),
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),    // r0 = 42
            Instr::abx(Opcode::LoadString, 1, 0).raw(), // r1 = "unused" (dead)
            Instr::abc(Opcode::Return1, 0, 0, 0).raw(), // return r0
        ],
        2,
    );
    let mir = lower(&art).expect("dead string load lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// An out-of-range constant index rejects (falls back) — never risks the VM's
/// out-of-range → nil semantics being miscompiled.
/// An out-of-range index (even a constant one) is no longer rejected: it takes
/// the dynamic `Maybe<Int>` path and returns `nil`, matching the VM. Codegen
/// emits the by-value `get_pair` call and the nil-or-value return branch.
#[test]
fn out_of_range_index_lowers_to_maybe_returning_nil() {
    use lk_core::vm::ConstHeapValueData;
    let consts = ConstPoolData {
        ints: vec![5],
        floats: Vec::new(),
        strings: Vec::new(),
        heap_values: vec![ConstHeapValueData::List(vec![
            ConstRuntimeValueData::Int(1),
            ConstRuntimeValueData::Int(2),
        ])],
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadHeapConst, 1, 0).raw(),
            Instr::abc(Opcode::Move, 0, 1, 0).raw(),
            Instr::abx(Opcode::LoadInt, 2, 0).raw(), // index 5, out of range
            Instr::abc(Opcode::GetList, 1, 0, 2).raw(),
            Instr::abc(Opcode::Return1, 1, 0, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("out-of-range index lowers via Maybe");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    // Present branch prints the element; absent branch prints nothing (just
    // the arena cleanup + `ret`), matching the VM's silent top-level nil return.
}

#[test]
fn lowers_straightline_integer_division() {
    let art = artifact(
        ints(vec![20, 4]),
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),
            Instr::abc(Opcode::DivInt, 2, 0, 1).raw(),
            Instr::abc(Opcode::Return, 2, 1, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

#[test]
fn lowers_early_return_conditional() {
    let art = artifact(
        ints(vec![3, 5]),
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),
            Instr::abc(Opcode::CmpLtInt, 2, 0, 1).raw(),
            Instr::as_bx(Opcode::BrFalse, 2, 1).raw(),
            Instr::abc(Opcode::Return, 0, 1, 0).raw(),
            Instr::abc(Opcode::Return, 1, 1, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert!(matches!(mir.functions[0].blocks[0].term, Term::CondBr { .. }));
}

#[test]
fn lowers_if_else_merge_with_phi() {
    let art = artifact(
        ints(vec![3, 5]),
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),
            Instr::abc(Opcode::CmpLtInt, 2, 0, 1).raw(),
            Instr::as_bx(Opcode::BrFalse, 2, 2).raw(),
            Instr::abc(Opcode::Move, 3, 0, 0).raw(),
            Instr::sj(Opcode::Jmp, 1).raw(),
            Instr::abc(Opcode::Move, 3, 1, 0).raw(),
            Instr::abc(Opcode::Return, 3, 1, 0).raw(),
        ],
        4,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    let merge = mir.functions[0]
        .blocks
        .iter()
        .find(|b| matches!(b.term, Term::Ret(Some(_))))
        .unwrap();
    assert_eq!(merge.params.len(), 1, "join block carries one phi param for r3");
}

#[test]
fn lowers_fused_compare_branch() {
    let art = artifact(
        ints(vec![3, 99]),
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),
            Instr::abc(Opcode::TestLeIntI, 0, 0, 5).raw(),
            Instr::sj(Opcode::Jmp, 1).raw(),
            Instr::abc(Opcode::Return, 0, 1, 0).raw(),
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),
            Instr::abc(Opcode::Return, 1, 1, 0).raw(),
        ],
        2,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
}

/// `s=0; i=1; while (i <= 5) { s += i; i += 1; } return s;` — a real loop with a
/// back-edge, exercising Braun loop-header phi construction. Sum 1..=5 = 15.
#[test]
fn lowers_counted_loop_with_backedge() {
    let art = artifact(
        ints(vec![0, 1]),
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),       // 0: s=0
            Instr::abx(Opcode::LoadInt, 1, 1).raw(),       // 1: i=1
            Instr::abc(Opcode::TestLeIntI, 1, 0, 5).raw(), // 2: test i<=5 (jump when false)
            Instr::sj(Opcode::Jmp, 3).raw(),               // 3: (fused) -> pc7 (exit)
            Instr::abc(Opcode::AddInt, 0, 0, 1).raw(),     // 4: s += i
            Instr::abc(Opcode::AddIntI, 1, 1, 1).raw(),    // 5: i += 1
            Instr::sj(Opcode::Jmp, -5).raw(),              // 6: -> pc2 (back-edge)
            Instr::abc(Opcode::Return, 0, 1, 0).raw(),     // 7: return s
        ],
        2,
    );
    let mir = lower(&art).expect("lowers loop");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    // The loop header (block containing the fused test) carries phi params for
    // the loop-carried s and i.
}

#[test]
fn lowers_float_arithmetic() {
    let art = artifact(
        floats(vec![1.5, 2.5]),
        vec![
            Instr::abx(Opcode::LoadFloat, 0, 0).raw(),
            Instr::abx(Opcode::LoadFloat, 1, 1).raw(),
            Instr::abc(Opcode::AddFloat, 2, 0, 1).raw(),
            Instr::abc(Opcode::Return, 2, 1, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert_eq!(mir.functions[0].ret, Ty::F64);
}

#[test]
fn int_arith_dispatches_to_float_on_float_operands() {
    // `AddInt` dispatches on runtime operand type: two floats → float add.
    let art = artifact(
        floats(vec![1.5, 2.5]),
        vec![
            Instr::abx(Opcode::LoadFloat, 0, 0).raw(),
            Instr::abx(Opcode::LoadFloat, 1, 1).raw(),
            Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),
            Instr::abc(Opcode::Return, 2, 1, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(lk_aot_mir::validate(&mir), Ok(()));
    assert_eq!(mir.functions[0].ret, Ty::F64);
}

#[test]
fn int_add_coerces_mixed_operands() {
    // int + float → the int operand is widened (`sitofp`) then float-added.
    let consts = ConstPoolData {
        ints: vec![10],
        floats: vec![2.5],
        strings: Vec::new(),
        heap_values: Vec::new(),
    };
    let art = artifact(
        consts,
        vec![
            Instr::abx(Opcode::LoadInt, 0, 0).raw(),
            Instr::abx(Opcode::LoadFloat, 1, 0).raw(),
            Instr::abc(Opcode::AddInt, 2, 0, 1).raw(),
            Instr::abc(Opcode::Return, 2, 1, 0).raw(),
        ],
        3,
    );
    let mir = lower(&art).expect("lowers");
    assert_eq!(mir.functions[0].ret, Ty::F64);
}
