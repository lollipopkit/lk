use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

#[test]
fn compiler_dynamic_method_helper_reads_runtime_properties() {
    let program = parse_program(
        r#"
        let user = {"score": 40};
        return user.score() + [1, 2].len() + "ok".len();
        "#,
    );
    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));

    let result = program.execute_with_ctx(&mut ctx).expect("execute program");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(44)]);
}

#[test]
fn compiler_dynamic_method_helper_calls_runtime_callable_property() {
    let program = parse_program(
        r#"
        fn add(a, b) {
            return a + b;
        }
        let table = {"add": add};
        return table.add(40, 2);
        "#,
    );
    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));

    let result = program.execute_with_ctx(&mut ctx).expect("execute program");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_dynamic_method_helper_calls_runtime_callable_property_with_named_args() {
    let program = parse_program(
        r#"
        fn add(a, {b: Int}) {
            return a + b;
        }
        let table = {"add": add};
        return table.add(40, b: 2);
        "#,
    );
    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));

    let result = program.execute_with_ctx(&mut ctx).expect("execute program");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_inlines_simple_direct_function_call_without_call_opcode() {
    let module = compile_source_module(
        r#"
        fn score(price, qty, discount) {
            let subtotal = price * qty;
            let fee = (subtotal % 17) + 3;
            return subtotal + fee - discount;
        }

        return score(7, 6, 3);
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[0];

    assert!(
        !entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::Call | Opcode::CallDirect)),
        "simple direct function call should inline in the caller"
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(50)]);
}

#[test]
fn compiler_inlines_direct_function_with_if_and_local_assignment() {
    let module = compile_source_module(
        r#"
        fn score(amount, prior) {
            let score = 0;
            if amount > 900 {
                score += 40;
            } else if amount > 400 {
                score += 15;
            }
            if prior > 0 {
                score += prior * 9;
            }
            return score;
        }

        return score(950, 2);
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[0];

    assert!(
        !entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::Call | Opcode::CallDirect)),
        "direct function with local if/assign prefix should inline in the caller"
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(58)]);
}

#[test]
fn compiler_inline_if_condition_reuses_readonly_local() {
    let module = compile_source_module(
        r#"
        let flag = true;

        fn choose(input) {
            if input {
                return 40;
            }
            return 2;
        }

        return choose(flag);
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[0];

    assert!(
        !entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::Call | Opcode::CallDirect)),
        "direct function with readonly if condition should inline in the caller"
    );
    let test_pc = entry
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode::Test | Opcode::BrFalse | Opcode::BrTrue))
        .expect("conditional branch");
    let condition = entry.code[test_pc].a();
    assert!(
        entry.performance.is_local_slot(condition as u16),
        "inline if should test the readonly local directly"
    );
    assert!(
        !entry.code[..test_pc]
            .iter()
            .any(|instr| instr.opcode() == Opcode::Move && instr.a() == condition),
        "inline if condition should not copy the local into a temporary"
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(40)]);
}

#[test]
fn compiler_inlines_direct_function_with_while_loop() {
    let module = compile_source_module(
        r#"
        fn gcd(a0, b0) {
            let a = a0;
            let b = b0;
            while (b != 0) {
                let t = a % b;
                a = b;
                b = t;
            }
            return a;
        }

        return gcd(1071, 462);
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[0];

    assert!(
        !entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::Call | Opcode::CallDirect)),
        "direct function with local while loop should inline in the caller"
    );
    let cmp_pc = entry
        .code
        .iter()
        .position(|instr| {
            matches!(
                instr.opcode(),
                Opcode::CmpNeInt | Opcode::TestNeInt | Opcode::TestNeIntI | Opcode::BrEqZeroInt | Opcode::BrNeZeroInt
            )
        })
        .expect("inlined gcd loop should compare/branch on b != 0");
    let loop_target = first_backward_jmp_target_after(entry, cmp_pc);
    if matches!(
        entry.code[cmp_pc].opcode(),
        Opcode::TestNeIntI | Opcode::BrEqZeroInt | Opcode::BrNeZeroInt
    ) {
        assert!(loop_target >= cmp_pc as i64);
    } else {
        let zero_load_pc = entry
            .code
            .iter()
            .enumerate()
            .take(cmp_pc)
            .rev()
            .find(|(_, instr)| instr.opcode() == Opcode::LoadInt)
            .map(|(pc, _)| pc)
            .expect("inlined gcd loop should load zero before compare");
        assert!(
            loop_target >= cmp_pc as i64,
            "inlined while LICM should skip LoadInt at {zero_load_pc}, but loop back targets {loop_target}"
        );
    }

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(21)]);
}

#[test]
fn compiler_inlines_direct_function_with_while_early_return() {
    let module = compile_source_module(
        r#"
        fn find_even(target, limit) {
            let lo = 0;
            let hi = limit - 1;
            while (lo <= hi) {
                let mid = (lo + hi) / 2;
                let value = mid * 2;
                if value == target {
                    return mid;
                }
                if value < target {
                    lo = mid + 1;
                } else {
                    hi = mid - 1;
                }
            }
            return -1;
        }

        return find_even(84, 100);
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[0];

    assert!(
        !entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::Call | Opcode::CallDirect)),
        "direct function with loop early return should inline in the caller"
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

fn first_backward_jmp_target_after(function: &Function, pc: usize) -> i64 {
    function
        .code
        .iter()
        .enumerate()
        .skip(pc + 1)
        .find_map(|(i, instr)| {
            if instr.opcode() != Opcode::Jmp {
                return None;
            }
            let offset = instr.sj_arg();
            (offset < 0).then_some(i as i64 + 1 + i64::from(offset))
        })
        .expect("expected backward Jmp after pc")
}

#[test]
fn compiler_keeps_recursive_direct_function_call_as_call_direct() {
    let module = compile_source_module(
        r#"
        fn countdown(n) {
            if n == 0 {
                return 42;
            }
            return countdown(n - 1);
        }

        return countdown(3);
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[0];

    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode::CallDirect),
        "recursive direct function call should not be inlined"
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_runs_direct_function_with_string_method() {
    let program = parse_program(
        r#"
        fn price(sku, base, tax) {
            let discount = 0;
            if sku.starts_with("pro") {
                discount = 1;
            }
            return base + tax - discount;
        }

        return price("pro", 49, 8);
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["__lk_call_method"]).expect("compile");
    let entry = module.entry_function().expect("entry");
    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode::CallDirect),
        "removing StringStartsWith must not degrade the outer price call away from CallDirect"
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::Call),
        "removing StringStartsWith must not force the outer price call through generic Call"
    );

    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));
    let result = program.execute_with_ctx(&mut ctx).expect("execute program");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(56)]);
}

#[test]
fn compiler_lowers_set_method_to_set_index_without_receiver_clone() {
    let function = compile_source(
        r#"
        let hist = {};
        let key = "b1";
        hist.set(key, 40 + 2);
        return hist.b1;
        "#,
    )
    .expect("compile source");
    let set_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::SetIndex)
        .expect("SetIndex");
    let set_instr = function.code[set_pc];

    assert!(
        function.performance.is_local_slot(set_instr.a() as u16),
        "method set should use the receiver local slot directly"
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_drops_set_method_nil_result_for_statement() {
    let function = compile_source(
        r#"
        let hist = {};
        hist.set("answer", 42);
        return hist.answer;
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK)),
        "method set statement should still lower to runtime set opcode"
    );
    assert_eq!(
        function
            .code
            .iter()
            .filter(|instr| instr.opcode() == Opcode::LoadNil)
            .count(),
        0,
        "discarded method set result should not materialize nil"
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_set_method_preserving_nil_result() {
    let function = compile_source(
        r#"
        let hist = {};
        let result = hist.set("answer", 42);
        return [result, hist.answer];
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK)),
        "method set should lower to runtime set opcode"
    );
    let result = execute(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list return");
    };
    let Some(crate::val::HeapValue::List(crate::val::TypedList::Mixed(values))) = result.state.heap.get(handle) else {
        panic!("expected mixed list return");
    };
    assert_eq!(values, &[crate::val::RuntimeVal::Nil, crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_folds_string_literal_len_to_int_load() {
    let function = compile_source(
        r#"
        return "/admin/users".len() + "é".len();
        "#,
    )
    .expect("compile source");

    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::Len),
        "string literal len should fold at compile time"
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::LoadString),
        "folded string literal len should not materialize the string"
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(13)]);
}

#[test]
fn compiler_lowers_push_method_to_list_push_without_list_concat() {
    let function = compile_source(
        r#"
        let values = [];
        values.push(40);
        values.push(2);
        return values[0] + values[1];
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::ListPush),
        "method push should lower to ListPush"
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::NewList),
        "method push should not materialize one-element lists"
    );
    let get_index_pc = function
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetList))
        .expect("GetIndex/GetList");
    let index_fact = function
        .performance
        .index_op(get_index_pc)
        .expect("GetIndex performance fact");
    assert_eq!(index_fact.target_kind, crate::vm::analysis::PerfIndexTargetKind::List);
    assert_eq!(index_fact.value_kind, crate::vm::analysis::PerfValueKind::Int);
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_push_method_does_not_consume_local_argument() {
    let function = compile_source(
        r#"
        let value = 42;
        let values = [];
        values.push(value);
        return [value, values[0]];
        "#,
    )
    .expect("compile source");
    let push_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::ListPush)
        .expect("ListPush");
    let move_fact = function
        .performance
        .container_move(push_pc)
        .expect("ListPush move fact");

    assert!(!move_fact.move_value, "method push must copy current local arguments");
    let result = execute(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list return");
    };
    let Some(crate::val::HeapValue::List(crate::val::TypedList::Int(values))) = result.state.heap.get(handle) else {
        panic!("expected int list return");
    };
    assert_eq!(values, &vec![42, 42]);
}

#[test]
fn compiler_set_method_does_not_consume_local_value_argument() {
    let function = compile_source(
        r#"
        let value = 42;
        let values = [0];
        values.set(0, value);
        return [value, values[0]];
        "#,
    )
    .expect("compile source");
    let set_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::SetIndex)
        .expect("SetIndex");
    let move_fact = function.performance.container_move(set_pc).expect("SetIndex move fact");

    assert!(
        !move_fact.move_value,
        "method set must copy current local value arguments"
    );
    let result = execute(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list return");
    };
    let Some(crate::val::HeapValue::List(crate::val::TypedList::Int(values))) = result.state.heap.get(handle) else {
        panic!("expected int list return");
    };
    assert_eq!(values, &vec![42, 42]);
}

#[test]
fn compiler_runs_starts_with_method_through_runtime_helper() {
    let program = parse_program(
        r#"
        let device = "emu-android";
        if device.starts_with("emu") {
            return 42;
        }
        return 0;
        "#,
    );

    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));
    let result = program.execute_with_ctx(&mut ctx).expect("execute program");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_split_join_methods_to_string_opcodes() {
    let function = compile_source(
        r#"
        let line = "a|b|c";
        return line.split("|").join(",").len();
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::StringSplit),
        "string split should lower to StringSplit"
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::ListJoin),
        "list join should lower to ListJoin"
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::Call),
        "split/join should not call through the runtime method helper"
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(5)]);
}

#[test]
fn compiler_elides_split_join_same_separator_len() {
    let function = compile_source(
        r#"
        let tenant = 7;
        let line = "ts=1|tenant=t${tenant}|status=ok";
        return line.split("|").join("|").len();
        "#,
    )
    .expect("compile source");

    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::StringSplit),
        "same-separator split/join len should not materialize split list: {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::ListJoin),
        "same-separator split/join len should not materialize joined string: {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::Len),
        "dynamic template string should lower to direct Len: {:?}",
        function.code
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(24)]);
}

#[test]
fn compiler_lowers_map_get_module_call_to_get_index() {
    let program = parse_program(
        r#"
        let hist = {};
        let key = "answer";
        hist.set(key, 42);
        return map.get(hist, key);
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"]).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode::GetIndex),
        "map.get should lower to GetIndex"
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::Call),
        "map.get should not call through the runtime callable bridge"
    );
}

#[test]
fn compiler_errors_on_map_get_missing_receiver_in_call() {
    let program = parse_program(
        r#"
        fn id(value) {
            return value;
        }
        return id(map.get("x"));
        "#,
    );
    let err = Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"])
        .expect_err("map.get missing receiver must not lower as method get");

    assert!(
        err.to_string().contains("map.get"),
        "expected map.get arity error, got {err}"
    );
}

#[test]
fn compiler_lowers_map_get_method_call_to_get_index() {
    let program = parse_program(
        r#"
        let hist = {};
        let key = "answer";
        hist.set(key, 42);
        return hist.get(key);
        "#,
    );
    let module = Compiler::compile_module(&program).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode::GetIndex),
        "map get method should lower to GetIndex"
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::Call),
        "map get method should not call through the runtime callable bridge"
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_map_get_method_directly_into_destination() {
    let program = parse_program(
        r#"
        let values = {"x": 42};
        let key = "x";
        let value = values.get(key);
        return value;
        "#,
    );
    let module = Compiler::compile_module(&program).expect("compile module");
    let function = module.entry_function().expect("entry function");

    let get = function
        .code
        .iter()
        .find(|instr| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetFieldK))
        .expect("expected map get method lowering");
    let value_return = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::Return1)
        .expect("expected single return");

    assert_eq!(
        get.a(),
        value_return.a(),
        "map get method should write directly into the destination local: {:?}",
        function.code
    );
    assert!(
        !function.code.windows(2).any(
            |window| matches!(window[0].opcode(), Opcode::GetIndex | Opcode::GetFieldK)
                && window[1].opcode() == Opcode::Move
        ),
        "map get method should not emit GetIndex/GetFieldK followed by a destination Move: {:?}",
        function.code
    );
}

#[test]
fn compiler_folds_const_map_get_literal_key() {
    let program = parse_program(
        r#"
        let hist = {"answer": 42};
        return map.get(hist, "answer");
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"]).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::GetIndex),
        "const map + literal key should fold to a scalar load: {:?}",
        entry.code
    );
    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode::LoadInt),
        "folded const map value should load the scalar result: {:?}",
        entry.code
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_folds_const_map_get_method_literal_key() {
    let program = parse_program(
        r#"
        let hist = {"answer": 42};
        return hist.get("answer");
        "#,
    );
    let module = Compiler::compile_module(&program).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::GetIndex),
        "const map + literal key method should fold to a scalar load: {:?}",
        entry.code
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::Call),
        "const map get method should not call through runtime helper: {:?}",
        entry.code
    );
    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode::LoadInt),
        "folded const map method value should load the scalar result: {:?}",
        entry.code
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_hoists_loop_const_map_get_folded_scalar_values() {
    let program = parse_program(
        r#"
        let levels = {"admin": 100};
        let total = 0;
        for i in 1..=5 {
            total += map.get(levels, "admin");
        }
        return total;
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"]).expect("compile module");
    let entry = module.entry_function().expect("entry");
    let admin_loads = entry
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::LoadInt && entry.consts.int(instr.bx()) == Some(100))
        .count();

    assert_eq!(
        admin_loads, 1,
        "folded const map value should be cached once for the loop: {:?}",
        entry.code
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::GetIndex),
        "const map + literal key should still fold away GetIndex: {:?}",
        entry.code
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(500)]);
}

#[test]
fn compiler_hoists_loop_const_map_get_method_folded_scalar_values() {
    let program = parse_program(
        r#"
        let levels = {"admin": 100};
        let total = 0;
        for i in 1..=5 {
            total += levels.get("admin");
        }
        return total;
        "#,
    );
    let module = Compiler::compile_module(&program).expect("compile module");
    let entry = module.entry_function().expect("entry");
    let admin_loads = entry
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::LoadInt && entry.consts.int(instr.bx()) == Some(100))
        .count();

    assert_eq!(
        admin_loads, 1,
        "folded const map method value should be cached once for the loop: {:?}",
        entry.code
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::GetIndex),
        "const map + literal key method should still fold away GetIndex: {:?}",
        entry.code
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::Call),
        "loop const map get method should not call through runtime helper: {:?}",
        entry.code
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(500)]);
}

#[test]
fn compiler_does_not_fold_const_map_get_after_mutation() {
    let program = parse_program(
        r#"
        let hist = {"answer": 42};
        hist.set("answer", 100);
        return map.get(hist, "answer");
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"]).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetFieldK)),
        "mutated const map local must keep runtime lookup semantics: {:?}",
        entry.code
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(100)]);
}

#[test]
fn compiler_does_not_fold_const_map_get_method_after_mutation() {
    let program = parse_program(
        r#"
        let hist = {"answer": 42};
        hist.set("answer", 100);
        return hist.get("answer");
        "#,
    );
    let module = Compiler::compile_module(&program).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetFieldK)),
        "mutated const map local method must keep runtime lookup semantics: {:?}",
        entry.code
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(100)]);
}

#[test]
fn compiler_does_not_fold_loop_local_mutated_empty_map_get() {
    let program = parse_program(
        r#"
        let total = 0;
        for r in 1..=3 {
            let hist = {};
            for i in 1..=3 {
                let prev = map.get(hist, "x");
                if prev == nil {
                    hist.set("x", 1);
                } else {
                    hist.set("x", prev + 1);
                }
            }
            total += map.get(hist, "x");
        }
        return total;
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"]).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetFieldK)),
        "loop-local mutable map lookups must remain dynamic: {:?}",
        entry.code
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(9)]);
}

#[test]
fn compiler_lowers_math_floor_of_int_to_identity() {
    let program = parse_program(
        r#"
        let x = 40;
        return math.floor(x + 2);
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["math"]).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::AddInt | Opcode::AddIntI)),
        "integer argument should be lowered before floor"
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode::Call),
        "math.floor(Int) should not call through the runtime callable bridge"
    );
}

#[test]
fn compiler_inline_arg_closure_promotion_survives_scope_restore() {
    // Lowering an inline call's closure argument promotes the captured caller
    // local to a cell *in place*; the promotion record must survive the
    // inline scope restore. Regression: the second call site re-promoted,
    // storing the old cell as the new cell's value (Int + Obj at runtime).
    let module = compile_source_module(
        r#"
        fn pick(h, n) {
            if n > 3 {
                return h(n);
            }
            return h(0);
        }

        let off = 7;
        let first = pick(|q| q + off, 10);
        let second = pick(|q| q + off, 1);
        return first * 100 + second;
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[0];

    let promotions = entry
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::StoreCellVal)
        .count();
    assert_eq!(promotions, 1, "the captured local must be boxed exactly once");

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(1707)]);
}

#[test]
fn compiler_inline_arguments_resolve_names_in_caller_scope() {
    // Inline argument expressions must lower before any parameter binds:
    // interleaving let `add2(1, a)` resolve the second argument against the
    // already-bound first parameter `a` (passing 1 instead of the caller's
    // 100). Regression: printed 2 instead of 101.
    let module = compile_source_module(
        r#"
        fn add2(a, b) {
            let t = a % 97;
            return t + b;
        }
        let a = 100;
        return add2(1, a);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(101)]);
}

#[test]
fn compiler_inline_readonly_param_survives_later_closure_promotion() {
    // A read-only `Var` argument binds its parameter to the local's register
    // directly; a later closure argument capturing the same local promotes
    // it in place. The promotion must happen *before* the binding (locals a
    // closure argument captures pre-promote), or the parameter aliases the
    // cell. Regression: "Add expected numbers or strings, got Obj and Int".
    let module = compile_source_module(
        r#"
        fn use2(a, g) {
            let t = a + 1;
            return g(t);
        }
        let y = 10;
        return use2(y, |q| q + y);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    // t = 10 + 1; g(11) = 11 + 10.
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(21)]);
}
