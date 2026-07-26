use super::*;
use crate::vm::ProgramExec;

#[test]
fn compiler_lowers_struct_literal_and_field_access() {
    let function = compile_source(
        r#"
        let user = User { name: "Ada", score: 42 };
        return user.score;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::NewObject),
        "expected NewObject in {:?}",
        function.code
    );
    let object_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::NewObject)
        .expect("NewObject");
    let object_base = function.code[object_pc].b() as u16;
    assert!(
        !function.code[..object_pc]
            .iter()
            .any(|instr| instr.opcode() == Opcode::Move
                && matches!(instr.a() as u16, dst if dst >= object_base && dst < object_base + 5)),
        "struct literal fields should lower directly into the object build window"
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_accepts_type_only_declarations_as_noop() {
    let function = compile_source(
        r#"
        struct Point { x: Int, y: Int }
        type Count = Int;
        trait Named { fn name() -> String; }
        let point = Point { x: 40, y: 2 };
        return point.x + point.y;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

/// Trait dispatch resolves through the method table the VM builds from
/// `Module::type_info`. This used to be written against the now-removed
/// `__lk_register_trait{,_impl}` builtins — i.e. it tested the registration
/// mechanism rather than the language feature; using real `trait`/`impl`
/// syntax exercises the path programs actually take.
#[test]
fn compiler_trait_method_dispatch_uses_registered_impl() {
    let program = parse_program(
        r#"
        trait Area { fn area(self) -> Int; }
        struct Rect { w: Int, h: Int }
        impl Area for Rect {
            fn area(self) -> Int {
                return self.w * self.h;
            }
        }
        let rect = Rect { w: 6, h: 7 };
        return rect.area();
        "#,
    );
    let mut ctx = crate::vm::VmContext::new().with_type_checker(Some(crate::typ::TypeChecker::new_strict()));

    let result = program.execute_with_ctx(&mut ctx).expect("execute program");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_rewritten_list_assignment_to_set_index() {
    let function = compile_source(
        r#"
        let values = [1, 2, 3];
        values[1] = 40 + 2;
        return values.1;
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK)),
        "expected runtime set opcode in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_rewritten_map_assignment_to_set_index() {
    let function = compile_source(
        r#"
        let values = {"a": 1};
        values["b"] = 42;
        return values.b;
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK)),
        "expected runtime set opcode in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_rewritten_object_assignment_to_set_index() {
    let function = compile_source(
        r#"
        let user = User { score: 1 };
        user.score = 42;
        return user.score;
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK)),
        "expected runtime set opcode in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_list_index_access() {
    let function = compile_source("return [7, 8, 9].1;").expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(8)]);
}

#[test]
fn compiler_reads_local_index_target_without_receiver_clone() {
    let function = compile_source(
        r#"
        let values = [40, 2];
        return values[0] + values[1];
        "#,
    )
    .expect("compile source");
    for instr in function.code.iter().filter(|instr| instr.opcode() == Opcode::GetIndex) {
        assert!(
            function.performance.is_local_slot(instr.b() as u16),
            "local index receiver should be read from its local slot"
        );
    }

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_in_membership_to_contains_opcode() {
    let function = compile_source(
        r#"
        let needle = 2;
        let list = [1, 2, 3];
        let list_hit = needle in list;
        let text_need = "bc";
        let text = "abcd";
        let text_hit = text_need in text;
        let map_key = "answer";
        let map = {"answer": 42};
        let map_hit = map_key in map;
        if (list_hit && text_hit && map_hit) {
            return 42;
        }
        return 0;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::Contains),
        "expected Contains in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_while_with_int_comparison() {
    let function = compile_source(
        r#"
        let i = 0;
        let sum = 0;
        while (i < 4) {
            sum = sum + i;
            i = i + 1;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(6)]);
}

#[test]
fn compiler_lowers_for_array_rest_pattern_to_slice_from() {
    let function = compile_source(
        r#"
        let total = 0;
        for [head, ..tail] in [[40, 1, 2]] {
            total = head + tail.1;
        }
        return total;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::SliceFrom),
        "expected SliceFrom in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_if_let_map_rest_binding_to_map_rest() {
    let function = compile_source(
        r#"
        let data = {"a": 40, "b": 2};
        if let {"a": a, ..rest} = data {
            return a + rest.b;
        }
        return 0;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::MapRest),
        "expected MapRest in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_direct_function_call_through_module() {
    let module = compile_source_module(
        r#"
        fn add(a, b) {
            return a + b;
        }

        return add(20, 22);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_direct_call_immediate_arithmetic_uses_callee_frame() {
    let module = compile_source_module(
        r#"
        fn classify(n) {
            for _ in 1..=0 {
            }
            let x = 3;
            return x + 1;
        }

        return classify(3);
        "#,
    )
    .expect("compile module");
    let classify = &module.functions[1];

    assert!(
        classify.code.iter().any(|instr| instr.opcode() == Opcode::AddIntI),
        "callee should use immediate arithmetic in its own frame: {:?}",
        classify.code
    );

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(4)]);
}

#[test]
fn compiler_lowers_recursive_function_call_through_module() {
    let module = compile_source_module(
        r#"
        fn fact(n) {
            if (n < 2) {
                return 1;
            }
            return n * fact(n - 1);
        }

        return fact(5);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(120)]);
}

#[test]
fn compiler_lowers_native_call_through_module() {
    fn native_add(args: NativeArgs<'_>, _runtime: &mut crate::vm::NativeRuntime<'_>) -> Result<crate::val::RuntimeVal> {
        let [crate::val::RuntimeVal::Int(lhs), crate::val::RuntimeVal::Int(rhs)] = args.as_slice() else {
            bail!("native_add expects two ints");
        };
        Ok(crate::val::RuntimeVal::Int(lhs + rhs))
    }

    let module = compile_source_module_with_natives(
        "return native_add(19, 23);",
        vec![NativeEntry {
            name: "native_add".to_string(),
            arity: 2,
            function: NativeFunction::Plain(native_add),
        }],
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_top_level_define_to_global_slot() {
    let module = compile_source_module(
        r#"
        answer := 40;
        fn read_answer() {
            return answer + 2;
        }
        return read_answer();
        "#,
    )
    .expect("compile module");

    assert_eq!(module.globals.len(), 2);
    assert_eq!(module.globals[0].name.as_ref(), "answer");
    assert_eq!(module.globals[1].name.as_ref(), "read_answer");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
    assert_eq!(result.state.globals[0], crate::val::RuntimeVal::Int(40));
    assert!(matches!(result.state.globals[1], crate::val::RuntimeVal::Obj(_)));
}

#[test]
fn compiler_keeps_top_level_let_in_entry_frame() {
    let module = compile_source_module(
        r#"
        let local = 40;
        answer := local + 2;
        return answer;
        "#,
    )
    .expect("compile module");

    assert_eq!(module.globals.len(), 1);
    assert_eq!(module.globals[0].name.as_ref(), "answer");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
    assert_eq!(result.state.globals[0], crate::val::RuntimeVal::Int(42));
}

#[test]
fn compiler_promotes_top_level_let_to_global_when_function_reads_it() {
    let module = compile_source_module(
        r#"
        let local = 40;
        fn read_local() {
            return local + 2;
        }
        return read_local();
        "#,
    )
    .expect("compile module");

    assert_eq!(module.globals.len(), 2);
    assert_eq!(module.globals[0].name.as_ref(), "local");
    assert_eq!(module.globals[1].name.as_ref(), "read_local");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
    assert_eq!(result.state.globals[0], crate::val::RuntimeVal::Int(40));
    assert!(matches!(result.state.globals[1], crate::val::RuntimeVal::Obj(_)));
}

#[test]
fn compiler_lowers_global_assignment_from_function() {
    let module = compile_source_module(
        r#"
        counter := 1;
        fn bump() {
            counter = counter + 41;
            return counter;
        }
        return bump();
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
    assert_eq!(result.state.globals[0], crate::val::RuntimeVal::Int(42));
    assert!(matches!(result.state.globals[1], crate::val::RuntimeVal::Obj(_)));
}

#[test]
fn compiler_lowers_closure_capturing_function_param() {
    let module = compile_source_module(
        r#"
        fn make(base) {
            return |value| base + value;
        }

        let add40 = make(40);
        return add40(2);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_nested_closure_captures() {
    let module = compile_source_module(
        r#"
        fn make(base) {
            return |scale| |value| base + value * scale;
        }

        let maker = make(10);
        let f = maker(8);
        return f(4);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_closure_calling_captured_callable_param() {
    let module = compile_source_module(
        r#"
        fn apply_twice(value, f) {
            return |extra| f(value + extra);
        }

        let add_one = |x| x + 1;
        let apply = apply_twice(40, add_one);
        return apply(1);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_mutable_closure_capture_to_upval_cell() {
    let module = compile_source_module(
        r#"
        fn make() {
            let value = 40;
            let bump = || {
                value = value + 1;
                return value;
            };
            bump();
            return bump();
        }

        return make();
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    let opcodes = module
        .functions
        .iter()
        .flat_map(|function| function.code.iter().map(|instr| instr.opcode()))
        .collect::<Vec<_>>();

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
    assert!(opcodes.contains(&Opcode::LoadCellVal));
    assert!(opcodes.contains(&Opcode::StoreCellVal));
}

#[test]
fn compiler_lowers_inlined_adjacent_assignment_chain_to_move2() {
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

        return gcd(120, 84);
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[module.entry as usize];

    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode::Move2),
        "inlined adjacent assignment chain should use Move2: {:?}",
        entry.code
    );

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(12)]);
}

#[test]
fn compiler_lowers_pair_immediate_equality_condition_to_single_test() {
    let function = compile_source(
        r#"
        let state = 1;
        let event = 2;
        if state == 1 && event == 2 {
            return 42;
        }
        return 0;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::TestEqIntI2),
        "pair immediate equality should use TestEqIntI2: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_mutable_capture_observes_outer_write_after_closure_creation() {
    let module = compile_source_module(
        r#"
        fn make() {
            let value = 1;
            let read = || value;
            value = 42;
            return read();
        }

        return make();
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_mutable_capture_is_shared_between_multiple_closures() {
    let module = compile_source_module(
        r#"
        fn make() {
            let value = 40;
            let inc = || {
                value = value + 1;
                return value;
            };
            let read = || value;
            inc();
            inc();
            return read();
        }

        return make();
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_named_args_to_normal_call_window() {
    let module = compile_source_module(
        r#"
        fn add({x: Int, y: Int}) {
            return x + y;
        }

        return add(y: 2, x: 40);
        "#,
    )
    .expect("compile module");

    let calls = module
        .functions
        .iter()
        .flat_map(|function| function.code.iter())
        .filter(|instr| matches!(instr.opcode(), Opcode::Call | Opcode::CallDirect))
        .collect::<Vec<_>>();
    assert!(
        !calls.is_empty(),
        "expected named-call lowering to reuse a positional call opcode"
    );
    assert!(
        calls
            .iter()
            .all(|instr| instr.opcode() == Opcode::CallDirect || instr.a() == instr.b()),
        "Call must use one window where callee slot is also return base"
    );

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_top_level_local_shadow_disables_direct_module_call() {
    let module = compile_source_module(
        r#"
        fn value() {
            return 1;
        }
        let value = || 42;
        return value();
        "#,
    )
    .expect("compile module");
    let entry = &module.functions[0];

    assert!(
        entry.code.iter().any(|instr| instr.opcode() == Opcode::Call),
        "shadowed function value must use normal callable dispatch"
    );
    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_default_named_args_in_plain_call() {
    let module = compile_source_module(
        r#"
        fn answer({x: Int? = 42}) {
            return x;
        }

        return answer();
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_named_default_that_references_positional_param() {
    let module = compile_source_module(
        r#"
        fn add(a, {b: Int? = a + 2}) {
            return b;
        }

        return add(40);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_named_default_that_references_earlier_named_param() {
    let module = compile_source_module(
        r#"
        fn add({a: Int? = 40, b: Int? = a + 2}) {
            return b;
        }

        return add();
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_mixed_positional_and_named_args() {
    let module = compile_source_module(
        r#"
        fn add(a, {b: Int}) {
            return a + b;
        }

        return add(40, b: 2);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_rejects_missing_required_named_args() {
    let err = compile_source_module(
        r#"
        fn add({x: Int}) {
            return x;
        }

        return add();
        "#,
    )
    .expect_err("missing named arg should fail at compile time");

    assert!(
        err.to_string().contains("missing required named argument `x`"),
        "{err:?}"
    );
}

#[test]
fn compiler_lowers_if_let_variable_and_literal_patterns() {
    let function = compile_source(
        r#"
        if let x = 41 {
            if let 0 = x {
                return 0;
            } else {
                return x + 1;
            }
        }
        return 0;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_if_let_range_guard_and_or_patterns() {
    let function = compile_source(
        r#"
        let age = 25;
        let status = 201;
        if let 18..65 = age {
            if let x if x > 20 = age {
                if let 200 | 201 | 202 = status {
                    return x + 17;
                }
            }
        }
        return 0;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_while_let_with_register_binding() {
    let function = compile_source(
        r#"
        let i = 0;
        while let x = i {
            if (x == 3) {
                break;
            }
            i += 1;
        }
        return i;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler_lowers_match_literal_and_binding_arms() {
    let function = compile_source(
        r#"
        let x = 41;
        let y = match x {
            0 => 0,
            value => value + 1,
        };
        return y;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_logical_nullish_optional_and_template_expressions() {
    let function = compile_source(
        r#"
        let x = false || true;
        let y = true && x;
        let z = nil ?? 41;
        let missing = nil?.answer;
        let text = "answer=${z + 1}";
        if (!y) {
            return 0;
        }
        if (!(missing == nil)) {
            return 0;
        }
        return text;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected heap string");
    };
    let crate::val::HeapValue::String(value) = result.state.heap.get(handle).expect("heap string") else {
        panic!("expected heap string");
    };
    assert_eq!(value.as_ref(), "answer=42");
}

#[test]
fn compiler_lowers_compound_assign_break_and_continue_in_while() {
    let function = compile_source(
        r#"
        let i = 0;
        let sum = 0;
        while (i < 10) {
            i += 1;
            if (i == 3) {
                continue;
            }
            if (i == 7) {
                break;
            }
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(18)]);
}

/// The global-use facts a cross-module dispatch reads
/// (`ImplMethod::{writes_globals, reads_globals}`). They are computed as a
/// post-pass, so what this pins is that the post-pass ran at all and that its
/// conservative side is on the safe end.
#[test]
fn impl_methods_record_how_their_subtree_uses_globals() {
    let module = crate::vm::Compiler::compile_source_module(
        r#"
        let counter = 0;
        fn bump() { counter = counter + 1; }
        fn twice(n: Int) -> Int { return n + n; }
        struct P { v: Int }
        trait T {
            fn pure(self) -> Int;
            fn reads(self) -> Int;
            fn writes(self) -> Int;
            fn indirect(self) -> Int;
        }
        impl T for P {
            fn pure(self) -> Int { return twice(self.v); }
            fn reads(self) -> Int { return self.v + counter; }
            fn writes(self) -> Int { counter = 7; return counter; }
            fn indirect(self) -> Int { let g = twice; return g(self.v); }
        }
        return 0;
        "#,
    )
    .expect("compile module");

    let method = |name: &str| {
        module
            .type_info
            .impls
            .iter()
            .flat_map(|decl| decl.methods.iter())
            .find(|method| method.name == name)
            .unwrap_or_else(|| panic!("impl method `{name}` present"))
    };

    // Calls a named function through `CallDirect`, which the walk follows.
    let pure = method("pure");
    assert!(!pure.writes_globals);
    assert!(pure.reads_globals.is_empty());

    // A read is fine and gets recorded: the dispatch seeds exactly this slot.
    let reads = method("reads");
    assert!(!reads.writes_globals);
    assert_eq!(reads.reads_globals.len(), 1, "`counter` is the one slot read");

    // A write cannot be supported across the boundary at all.
    assert!(method("writes").writes_globals);

    // `g` is a function *value* in a register, so the call is a generic `Call`
    // and the walk cannot see where it goes — conservatively "writes", which is
    // what keeps `reads_globals` complete for everything it clears.
    let indirect = method("indirect");
    assert!(indirect.writes_globals);
    assert!(
        indirect.reads_globals.is_empty(),
        "an unresolvable subtree reports no read list rather than a partial one"
    );

    // The lookup the dispatch path uses must find the same declaration.
    assert_eq!(
        module
            .type_info
            .method_by_function(pure.function)
            .map(|m| m.name.as_str()),
        Some("pure")
    );
}

/// A function body's block scope *is* the function scope, so an annotated local
/// is still known when the declared return type is validated.
///
/// It was not: the body was checked through `Stmt::Block`, which pushes a scope
/// and pops it, so `collect_return_types` inferred `return r` with `r` unknown
/// and reported "expected Int, got 'T0" for perfectly well-typed code.
#[test]
fn annotated_local_is_visible_to_the_declared_return_type() {
    let ok = crate::syntax::parse_program_source(
        "fn f() -> Int { let r: Int = 0; r = 7; return r; }\nreturn f();\n",
        crate::syntax::ParseOptions::default(),
    )
    .expect("parse");
    let mut tc = crate::typ::TypeChecker::new();
    for stmt in &ok.statements {
        stmt.type_check(&mut tc)
            .expect("an annotated local satisfies the declared return type");
    }

    // Every nested body too: returns are collected as each one is checked, so a
    // local declared inside an `if`/`while`/`for`/`try` is still in scope when its
    // `return` is inferred. A traversal after the body saw them all popped.
    for nested in [
        "if (1 == 1) { let r: Int = 7; return r; } return 0;",
        "while (1 == 1) { let r: Int = 7; return r; } return 0;",
        "for i in 0..1 { let r: Int = 7; return r; } return 0;",
        "try { let r: Int = 7; return r; } catch e { return 0; }",
    ] {
        let src = format!("fn f() -> Int {{ {nested} }}\nreturn f();\n");
        let program = crate::syntax::parse_program_source(&src, crate::syntax::ParseOptions::default()).expect("parse");
        let mut tc = crate::typ::TypeChecker::new();
        program
            .statements
            .iter()
            .try_for_each(|stmt| stmt.type_check(&mut tc))
            .unwrap_or_else(|err| panic!("`{nested}` should type-check: {err}"));
    }

    // Still rejects a genuinely wrong return, from any nesting depth.
    for bad in [
        "fn f() -> Int { let r: Int = 0; return \"s\"; }\nreturn 0;\n",
        "fn f() -> Int { try { return \"s\"; } catch e { return 1; } }\nreturn 0;\n",
        "fn f() -> Int { if (1 == 1) { return \"s\"; } return 0; }\nreturn 0;\n",
        "fn f() -> Int { for i in 0..1 { return \"s\"; } return 0; }\nreturn 0;\n",
        "fn f() -> Int { while (1 == 1) { return \"s\"; } return 0; }\nreturn 0;\n",
    ] {
        let program = crate::syntax::parse_program_source(bad, crate::syntax::ParseOptions::default()).expect("parse");
        let mut tc = crate::typ::TypeChecker::new();
        let err = program
            .statements
            .iter()
            .try_for_each(|stmt| stmt.type_check(&mut tc))
            .expect_err("a wrong return type must still be rejected");
        assert!(err.to_string().contains("Return type mismatch"), "got: {err}");
    }
}

/// A destructuring `let` binds each name to *its own* element type.
///
/// It used to bind the whole right-hand side to every name, so `v` in
/// `let [ok, v] = pick()` was typed as the entire tuple — invisible only while
/// inference was too coarse to produce a precise enough right-hand side.
#[test]
fn destructuring_let_distributes_the_pattern_over_the_type() {
    let check = |src: &str| -> anyhow::Result<()> {
        let program = crate::syntax::parse_program_source(src, crate::syntax::ParseOptions::default()).expect("parse");
        let mut tc = crate::typ::TypeChecker::new();
        program.statements.iter().try_for_each(|stmt| stmt.type_check(&mut tc))
    };

    // `Tuple<Bool, String>` gives `v: String` — usable as a string…
    check(
        "fn pick() -> Tuple<Bool, String> { return [true, \"x\"]; }\n\
         let [ok, v] = pick();\n\
         let s: String = v;\n\
         return 0;\n",
    )
    .expect("the second element is a String");

    // …and *only* as a string.
    let err = check(
        "fn pick() -> Tuple<Bool, String> { return [true, \"x\"]; }\n\
         let [ok, v] = pick();\n\
         let n: Int = v;\n\
         return 0;\n",
    )
    .expect_err("an element's type is now checked");
    assert!(err.to_string().contains("Int"), "got: {err}");

    // A `List<T>` distributes its element type to every position, and `..rest`
    // keeps the container shape.
    check(
        "fn nums() -> List<Int> { return [1, 2, 3]; }\n\
         let [a, ..tail] = nums();\n\
         let x: Int = a;\n\
         let t: List<Int> = tail;\n\
         return 0;\n",
    )
    .expect("list elements and tail distribute");
}

/// `Tuple<..>` in an annotation is the `Tuple` type, not a user generic that
/// merely *displays* the same.
///
/// `Type::parse` handled `List`/`Map`/`Set`/`Task`/`Channel` but not `Tuple`, so
/// the annotation became `Generic { name: "Tuple" }` while a heterogeneous list
/// literal infers `Type::Tuple` — hence "expected Tuple<Bool, String>, got
/// Tuple<Bool, String>".
#[test]
fn tuple_annotation_parses_as_the_tuple_type() {
    assert_eq!(
        crate::val::Type::parse("Tuple<Bool, String>"),
        Some(crate::val::Type::Tuple(vec![
            crate::val::Type::Bool,
            crate::val::Type::String
        ]))
    );
}

/// `#[export]` names a compiled function for the native backend.
///
/// The VM ignores the attribute, so the only place a mistake shows up is a
/// missing or misnamed symbol at link time — far from the source. These check
/// the name reaches `Function::export_name`, which is what codegen reads.
#[test]
fn export_attribute_defaults_to_the_source_name() {
    let module = compile_module(&parse_program(
        r#"
        #[export]
        fn tick() { return 1; }
        return tick();
        "#,
    ))
    .expect("compile module");
    let exported: Vec<_> = module
        .functions
        .iter()
        .filter_map(|function| function.export_name.as_deref())
        .collect();
    assert_eq!(exported, vec!["tick"]);
}

#[test]
fn export_attribute_takes_an_explicit_symbol() {
    let module = compile_module(&parse_program(
        r#"
        #[export("timer_isr")]
        fn tick() { return 1; }
        return tick();
        "#,
    ))
    .expect("compile module");
    let exported: Vec<_> = module
        .functions
        .iter()
        .filter_map(|function| function.export_name.as_deref())
        .collect();
    assert_eq!(exported, vec!["timer_isr"]);
}

/// A function with no `#[export]` stays internal — otherwise every function in
/// a program would land in the symbol table.
#[test]
fn functions_are_not_exported_by_default() {
    let module = compile_module(&parse_program(
        r#"
        fn tick() { return 1; }
        return tick();
        "#,
    ))
    .expect("compile module");
    assert!(module.functions.iter().all(|function| function.export_name.is_none()));
}

/// A malformed `#[export]` is an error rather than a silently ignored
/// attribute: ignoring it produces an undefined symbol somewhere unrelated.
#[test]
fn malformed_export_attribute_is_rejected() {
    for source in [
        "#[export(timer_isr)]\nfn tick() { return 1; }\nreturn tick();",
        "#[export(\"\")]\nfn tick() { return 1; }\nreturn tick();",
        "#[export(1)]\nfn tick() { return 1; }\nreturn tick();",
    ] {
        let error = compile_module(&parse_program(source)).expect_err("malformed export must fail");
        assert!(
            error.to_string().contains("export"),
            "unexpected error for {source:?}: {error}"
        );
    }
}

/// A top-level `let`/`const` used inside a function's `for` body is still a
/// global.
///
/// Whether a top-level binding becomes a module global is decided by scanning
/// each function for free variables. That scan had no `for` arm, so a name used
/// only inside a loop body was invisible: the binding stayed a local of the
/// entry function and the reference compiled to "undefined local/global" —
/// pointing at the use, with nothing to say the loop was what hid it.
///
/// One case per statement kind that owns a body, because the same omission is
/// possible in each and none of them fails loudly.
#[test]
fn top_level_bindings_are_visible_through_every_nested_body() {
    for (kind, source) in [
        (
            "for",
            "const A = 5;\nfn f() -> Int { let s = 0; for i in 0..3 { s = s + A; } return s; }\nreturn f();\n",
        ),
        (
            "while",
            "const A = 5;\nfn f() -> Int { let s = 0; let i = 0; while (i < 3) { s = s + A; i = i + 1; } return s; }\nreturn f();\n",
        ),
        (
            "if",
            "const A = 15;\nfn f() -> Int { if (1 < 2) { return A; } return 0; }\nreturn f();\n",
        ),
        (
            "try",
            "const A = 15;\nfn f() -> Int { try { return A; } catch e { return 0; } }\nreturn f();\n",
        ),
    ] {
        let module = compile_module(&parse_program(source)).unwrap_or_else(|error| panic!("{kind}: {error}"));
        let result = execute_module(&module).unwrap_or_else(|error| panic!("{kind}: {error}"));
        assert_eq!(
            result.returns,
            vec![crate::val::RuntimeVal::Int(15)],
            "{kind} body did not see the top-level binding"
        );
    }
}

/// A top-level `const` is a module global even when this file never reads it.
///
/// Whether a top-level binding is promoted is decided by scanning the file's
/// functions for free variables — which is right for `let` (a script's local)
/// and wrong for `const`. A module that declares register numbers *for its
/// importers* uses none of them itself, so they stayed entry-locals, never
/// reached the export map, and `use { REG } from "…"` failed with "not found
/// in runtime module" — pointing at the import rather than at the rule.
#[test]
fn unread_top_level_consts_are_still_module_globals() {
    let module = compile_module(&parse_program(
        "const EXPORTED = 0x3f8;\nlet unused_let = 1;\nfn f() -> Int { return 1; }\nreturn f();\n",
    ))
    .expect("compile module");
    let globals: Vec<&str> = module.globals.iter().map(|slot| slot.name.as_ref()).collect();
    assert!(
        globals.contains(&"EXPORTED"),
        "a top-level const must be a module global: {globals:?}"
    );
    // `let` keeps the old rule: nothing reads it, so it stays a local.
    assert!(
        !globals.contains(&"unused_let"),
        "an unread top-level `let` should not become a global: {globals:?}"
    );
}
