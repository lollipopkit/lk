use super::*;

#[test]
fn compiler32_dynamic_method_helper_reads_runtime_properties() {
    let program = parse_program32(
        r#"
        let user = {"score": 40};
        return user.score() + [1, 2].len() + "ok".len();
        "#,
    );
    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));

    let result = program.execute32_with_ctx(&mut ctx).expect("execute program");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(44)]);
}

#[test]
fn compiler32_dynamic_method_helper_calls_runtime_callable_property() {
    let program = parse_program32(
        r#"
        fn add(a, b) {
            return a + b;
        }
        let table = {"add": add};
        return table.add(40, 2);
        "#,
    );
    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));

    let result = program.execute32_with_ctx(&mut ctx).expect("execute program");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_dynamic_method_helper_calls_runtime_callable_property_with_named_args() {
    let program = parse_program32(
        r#"
        fn add(a, {b: Int}) {
            return a + b;
        }
        let table = {"add": add};
        return table.add(40, b: 2);
        "#,
    );
    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));

    let result = program.execute32_with_ctx(&mut ctx).expect("execute program");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_inlines_simple_direct_function_call_without_call_opcode() {
    let module = compile_source_module32(
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
            .any(|instr| matches!(instr.opcode(), Opcode32::Call | Opcode32::CallDirect)),
        "simple direct function call should inline in the caller"
    );

    let result = execute_module32(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(50)]);
}

#[test]
fn compiler32_inlines_direct_function_with_if_and_local_assignment() {
    let module = compile_source_module32(
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
            .any(|instr| matches!(instr.opcode(), Opcode32::Call | Opcode32::CallDirect)),
        "direct function with local if/assign prefix should inline in the caller"
    );

    let result = execute_module32(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(58)]);
}

#[test]
fn compiler32_inlines_direct_function_with_while_loop() {
    let module = compile_source_module32(
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
            .any(|instr| matches!(instr.opcode(), Opcode32::Call | Opcode32::CallDirect)),
        "direct function with local while loop should inline in the caller"
    );

    let result = execute_module32(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(21)]);
}

#[test]
fn compiler32_inlines_direct_function_with_while_early_return() {
    let module = compile_source_module32(
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
            .any(|instr| matches!(instr.opcode(), Opcode32::Call | Opcode32::CallDirect)),
        "direct function with loop early return should inline in the caller"
    );

    let result = execute_module32(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_keeps_recursive_direct_function_call_as_call_direct() {
    let module = compile_source_module32(
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
        entry.code.iter().any(|instr| instr.opcode() == Opcode32::CallDirect),
        "recursive direct function call should not be inlined"
    );

    let result = execute_module32(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_inlines_direct_function_with_map_get_and_string_intrinsic() {
    let program = parse_program32(
        r#"
        fn price(sku, region, prices, rates) {
            let base = map.get(prices, sku);
            let tax = map.get(rates, region);
            let discount = 0;
            if sku.starts_with("pro") {
                discount = 1;
            }
            return base + tax - discount;
        }

        let prices = {"basic": 19, "pro": 49};
        let rates = {"us": 8};
        return price("pro", "us", prices, rates);
        "#,
    );
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"]).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        !entry
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode32::Call | Opcode32::CallDirect)),
        "direct function with map.get and string intrinsic should inline in the caller"
    );
    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode32::GetIndex),
        "inlined map.get should lower to GetIndex"
    );
    assert!(
        entry
            .code
            .iter()
            .any(|instr| instr.opcode() == Opcode32::StringStartsWith),
        "inlined starts_with should lower to StringStartsWith"
    );
    assert!(
        !entry.code.iter().enumerate().any(|(pc, instr)| {
            instr.opcode() == Opcode32::Move
                && entry
                    .performance
                    .register_copy(pc)
                    .is_some_and(|fact| !fact.move_source)
                && entry
                    .performance
                    .register(instr.b() as u16)
                    .is_some_and(|fact| matches!(fact.value.kind, crate::vm::analysis::PerfValueKind::Map))
        }),
        "readonly inline map params should stay aliased instead of cloned into inline locals"
    );

    let result = execute_module32(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(56)]);
}

#[test]
fn compiler32_lowers_set_method_to_set_index_without_receiver_clone() {
    let function = compile_source32(
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
        .position(|instr| instr.opcode() == Opcode32::SetIndex)
        .expect("SetIndex");
    let set_instr = function.code[set_pc];

    assert!(
        function.performance.is_local_slot(set_instr.a() as u16),
        "method set should use the receiver local slot directly"
    );
    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_lowers_set_method_preserving_nil_result() {
    let function = compile_source32(
        r#"
        let hist = {};
        let result = hist.set("answer", 42);
        return [result, hist.answer];
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode32::SetIndex),
        "method set should lower to SetIndex"
    );
    let result = execute32(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list return");
    };
    let Some(crate::val::HeapValue::List(crate::val::TypedList::Mixed(values))) = result.state.heap.get(handle) else {
        panic!("expected mixed list return");
    };
    assert_eq!(values, &[crate::val::RuntimeVal::Nil, crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_lowers_push_method_to_list_push_without_list_concat() {
    let function = compile_source32(
        r#"
        let values = [];
        values.push(40);
        values.push(2);
        return values[0] + values[1];
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode32::ListPush),
        "method push should lower to ListPush"
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode32::NewList),
        "method push should not materialize one-element lists"
    );
    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_push_method_does_not_consume_local_argument() {
    let function = compile_source32(
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
        .position(|instr| instr.opcode() == Opcode32::ListPush)
        .expect("ListPush");
    let move_fact = function
        .performance
        .container_move(push_pc)
        .expect("ListPush move fact");

    assert!(!move_fact.move_value, "method push must copy current local arguments");
    let result = execute32(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list return");
    };
    let Some(crate::val::HeapValue::List(crate::val::TypedList::Int(values))) = result.state.heap.get(handle) else {
        panic!("expected int list return");
    };
    assert_eq!(values, &vec![42, 42]);
}

#[test]
fn compiler32_set_method_does_not_consume_local_value_argument() {
    let function = compile_source32(
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
        .position(|instr| instr.opcode() == Opcode32::SetIndex)
        .expect("SetIndex");
    let move_fact = function.performance.container_move(set_pc).expect("SetIndex move fact");

    assert!(
        !move_fact.move_value,
        "method set must copy current local value arguments"
    );
    let result = execute32(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list return");
    };
    let Some(crate::val::HeapValue::List(crate::val::TypedList::Int(values))) = result.state.heap.get(handle) else {
        panic!("expected int list return");
    };
    assert_eq!(values, &vec![42, 42]);
}

#[test]
fn compiler32_lowers_starts_with_method_to_string_opcode() {
    let function = compile_source32(
        r#"
        let device = "emu-android";
        if device.starts_with("emu") {
            return 42;
        }
        return 0;
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .any(|instr| instr.opcode() == Opcode32::StringStartsWith),
        "string starts_with should lower to StringStartsWith"
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode32::Call),
        "string starts_with should not call through the runtime method helper"
    );
    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_lowers_split_join_methods_to_string_opcodes() {
    let function = compile_source32(
        r#"
        let line = "a|b|c";
        return line.split("|").join("|").len();
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .any(|instr| instr.opcode() == Opcode32::StringSplit),
        "string split should lower to StringSplit"
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode32::ListJoin),
        "list join should lower to ListJoin"
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode32::Call),
        "split/join should not call through the runtime method helper"
    );
    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(5)]);
}

#[test]
fn compiler32_lowers_map_get_module_call_to_get_index() {
    let program = parse_program32(
        r#"
        let hist = {};
        let key = "answer";
        hist.set(key, 42);
        return map.get(hist, key);
        "#,
    );
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"]).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode32::GetIndex),
        "map.get should lower to GetIndex"
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode32::Call),
        "map.get should not call through the runtime callable bridge"
    );
}

#[test]
fn compiler32_lowers_math_floor_of_int_to_identity() {
    let program = parse_program32(
        r#"
        let x = 40;
        return math.floor(x + 2);
        "#,
    );
    let module =
        Compiler32::compile_module_with_natives_and_globals(&program, Vec::new(), ["math"]).expect("compile module");
    let entry = module.entry_function().expect("entry");

    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode32::AddInt),
        "integer argument should be lowered before floor"
    );
    assert!(
        !entry.code.iter().any(|instr| instr.opcode() == Opcode32::Call),
        "math.floor(Int) should not call through the runtime callable bridge"
    );
}
