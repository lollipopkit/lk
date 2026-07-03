use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;
use crate::vm::{NativeArgs, NativeFunction, execute, execute_module};
use crate::{stmt::stmt_parser::StmtParser, token::Tokenizer};

fn parse_program(source: &str) -> crate::stmt::Program {
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let mut parser = StmtParser::new(&tokens);
    parser.parse_program().expect("parse program")
}

mod arithmetic;
mod call_intrinsics;
mod loops;
mod patterns;
mod template;

#[test]
fn compiler_lowers_int_arithmetic_to_executable_function() {
    let expr = Expr::Bin(
        Box::new(Expr::Literal(LiteralVal::Int(8))),
        BinOp::Mul,
        Box::new(Expr::Literal(LiteralVal::Int(7))),
    );

    let function = compile_expr(&expr).expect("compile");
    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(56)]);
}

#[test]
fn compiler_lowers_float_arithmetic_to_typed_instr() {
    let add_function = compile_source(
        r#"
        let x = 1.5;
        return x + 2.0;
        "#,
    )
    .expect("compile add");

    assert!(
        add_function.code.iter().any(|instr| instr.opcode() == Opcode::AddFloat),
        "expected AddFloat in {:?}",
        add_function.code
    );

    let add_result = execute(&add_function).expect("execute add");
    assert_eq!(add_result.returns, vec![crate::val::RuntimeVal::Float(3.5)]);

    let mul_expr = Expr::Bin(
        Box::new(Expr::Bin(
            Box::new(Expr::Literal(LiteralVal::Float(1.5))),
            BinOp::Mul,
            Box::new(Expr::Literal(LiteralVal::Float(4.0))),
        )),
        BinOp::Div,
        Box::new(Expr::Literal(LiteralVal::Float(2.0))),
    );
    let function = compile_expr(&mul_expr).expect("compile mul/div");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::MulFloat),
        "expected MulFloat in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::DivFloat),
        "expected DivFloat in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Float(3.0)]);
}

#[test]
fn compiler_uses_register_facts_for_compound_float_arithmetic() {
    let function = compile_source(
        r#"
        let x = 1.5;
        x += 2.0;
        return x;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::AddFloat),
        "expected register facts to select AddFloat in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Float(3.5)]);
}

#[test]
fn compiler_records_local_slots_and_local_copy_facts() {
    let function = compile_source(
        r#"
        let x = 1;
        x = x + 1;
        return x;
        "#,
    )
    .expect("compile source");

    assert!(
        function.performance.is_local_slot(0),
        "direct-to-slot lowering should keep the local slot fact"
    );
    assert!(
        function
            .code
            .iter()
            .enumerate()
            .all(|(pc, instr)| instr.opcode() != Opcode::Move || function.performance.local_copy(pc).is_none()),
        "literal and binary local writes should not emit local-copy moves: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(2)]);
}

#[test]
fn compiler_lowers_short_and_long_strings() {
    let short = compile_expr(&Expr::Literal(LiteralVal::from_str("short"))).expect("compile short");
    let long = compile_expr(&Expr::Literal(LiteralVal::from_str("longer-than-seven"))).expect("compile long");

    let short_result = execute(&short).expect("execute short");
    let long_result = execute(&long).expect("execute long");

    assert_eq!(short_result.returns[0].kind(), crate::val::RuntimeValKind::ShortStr);
    assert!(short.consts.heap_values.is_empty());
    assert_eq!(long.consts.heap_values.len(), 1);
    assert_eq!(long_result.returns[0].kind(), crate::val::RuntimeValKind::Obj);
    assert_eq!(long_result.state.heap.len(), 1);
}

#[test]
fn compiler_lowers_literal_list_and_map_values_to_heap_consts() {
    let list = compile_expr(&Expr::List(vec![
        Box::new(Expr::Literal(LiteralVal::Int(1))),
        Box::new(Expr::Literal(LiteralVal::from_str("longer-than-seven"))),
    ]))
    .expect("compile list");
    let map = compile_expr(&Expr::Map(vec![(
        Box::new(Expr::Literal(LiteralVal::from_str("answer"))),
        Box::new(Expr::Literal(LiteralVal::Int(42))),
    )]))
    .expect("compile map");

    assert_eq!(list.consts.heap_values.len(), 1);
    assert_eq!(map.consts.heap_values.len(), 1);
    assert!(
        list.code
            .iter()
            .any(|instr| instr.opcode() == crate::vm::Opcode::LoadHeapConst)
    );
    assert!(
        map.code
            .iter()
            .any(|instr| instr.opcode() == crate::vm::Opcode::LoadHeapConst)
    );

    let list_result = execute(&list).expect("execute list");
    let map_result = execute(&map).expect("execute map");

    let crate::val::RuntimeVal::Obj(list_handle) = list_result.returns[0] else {
        panic!("expected heap list");
    };
    let crate::val::HeapValue::List(crate::val::TypedList::Mixed(values)) =
        list_result.state.heap.get(list_handle).expect("list")
    else {
        panic!("expected mixed list");
    };
    assert_eq!(values[0], crate::val::RuntimeVal::Int(1));

    let crate::val::RuntimeVal::Obj(map_handle) = map_result.returns[0] else {
        panic!("expected heap map");
    };
    let crate::val::HeapValue::Map(crate::val::TypedMap::StringInt(values)) =
        map_result.state.heap.get(map_handle).expect("map")
    else {
        panic!("expected string-int map");
    };
    assert_eq!(values.get("answer"), Some(&42));
}

#[test]
fn compiler_lowers_program_locals_and_assignment() {
    let function = compile_source(
        r#"
        let x = 40;
        x = x + 2;
        return x;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_elides_compound_assignment_self_move() {
    let function = compile_source(
        r#"
        let total = 0;
        total += 1;
        total += 2;
        return total;
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .all(|instr| instr.opcode() != Opcode::Move || instr.a() != instr.b()),
        "compound assignment should not emit self Move: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler_lowers_int_min_max_update_if() {
    let function = compile_source(
        r#"
        let min_price = 100;
        let price = 42;
        if price < min_price {
            min_price = price;
        }
        let best = 7;
        let profit = 19;
        if profit > best {
            best = profit;
        }
        return min_price + best;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::MinInt),
        "min update should lower to MinInt: {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::MaxInt),
        "max update should lower to MaxInt: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(61)]);
}

#[test]
fn compiler_lowers_zero_int_conditions_to_direct_branches() {
    let function = compile_source(
        r#"
        let a = 25;
        let b = 15;
        while (b != 0) {
            let t = a % b;
            a = b;
            b = t;
        }
        let marker = 0;
        if (a == 0) {
            marker += 100;
        }
        if (a != 0) {
            marker += 7;
        }
        return (a * 10) + marker;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::BrEqZeroInt),
        "not-equal-zero false edge should lower to BrEqZeroInt: {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::BrNeZeroInt),
        "equal-zero false edge should lower to BrNeZeroInt: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(57)]);
}

#[test]
fn compiler_lowers_small_int_conditions_to_direct_branches() {
    let function = compile_source(
        r#"
        let state = 2;
        let event = 3;
        let score = 0;
        if (state == 2) {
            score += 10;
        } else {
            score += 100;
        }
        if (event != 3) {
            score += 1000;
        } else {
            score += 7;
        }
        while (state != 5) {
            state += 1;
        }
        return score + state;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::BrEqIntI4),
        "not-equal-small-int false edge should lower to BrEqIntI4: {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::BrNeIntI4),
        "equal-small-int false edge should lower to BrNeIntI4: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(22)]);
}

#[test]
fn compiler_lowers_small_int_modulo_zero_conditions_to_direct_branches() {
    let function = compile_source(
        r#"
        let i = 1;
        let score = 0;
        while (i != 18) {
            if ((i % 3) == 0) {
                score += 10;
            }
            if ((i % 5) != 0) {
                score += 1;
            }
            i += 1;
        }
        return score;
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .any(|instr| instr.opcode() == Opcode::BrModNeZeroIntI4),
        "equal-modulo-zero false edge should lower to BrModNeZeroIntI4: {:?}",
        function.code
    );
    assert!(
        function
            .code
            .iter()
            .any(|instr| instr.opcode() == Opcode::BrModEqZeroIntI4),
        "not-equal-modulo-zero false edge should lower to BrModEqZeroIntI4: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(64)]);
}

#[test]
fn compiler_sinks_default_assign_into_if_chain_else() {
    let function = compile_source(
        r#"
        let state = 2;
        let event = 1;
        if (state == 2) {
            event = 3;
        } else if (state == 3) {
            event = 4;
        }
        return event;
        "#,
    )
    .expect("compile source");

    let first_branch = function
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode::BrNeIntI4 | Opcode::TestEqIntI))
        .expect("expected first state branch");
    let event_default_write = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::LoadInt && instr.a() == 1)
        .expect("expected default event write in final else");
    assert!(
        event_default_write > first_branch,
        "default write should be sunk after the branch chain: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler_rebinds_simple_local_assignment_without_move() {
    let function = compile_source(
        r#"
        let a = 1;
        let b = 2;
        a = b;
        return a + b;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().all(|instr| instr.opcode() != Opcode::Move),
        "simple local assignment should rebind locals instead of emitting Move: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(4)]);
}

#[test]
fn compiler_rebind_copy_on_write_preserves_local_assignment_semantics() {
    let function = compile_source(
        r#"
        let a = 1;
        let b = 2;
        a = b;
        a += 1;
        return b * 10 + a;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(23)]);
}

#[test]
fn compiler_rebind_copy_on_write_preserves_redefined_local_semantics() {
    let function = compile_source(
        r#"
        let a = 1;
        let b = 2;
        a = b;
        let b = 3;
        return a * 10 + b;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(23)]);
}

#[test]
fn compiler_lowers_adjacent_local_rotation_to_move2() {
    let function = compile_source(
        r#"
        let a = 10;
        let b = 20;
        let t = 30;
        {
            a = b;
            b = t;
        }
        return a * 100 + b;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::Move2),
        "adjacent local rotation should use Move2: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(2030)]);
}

#[test]
fn compiler_move2_preserves_sequential_assignment_semantics() {
    let function = compile_source(
        r#"
        let a = 10;
        let b = 20;
        {
            a = b;
            b = a;
        }
        return a * 100 + b;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::Move2),
        "adjacent self-source assignment should use Move2: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(2020)]);
}

#[test]
fn compiler_block_assignment_preserves_outer_local_semantics() {
    let function = compile_source(
        r#"
        let a = 1;
        let b = 42;
        {
            a = b;
        }
        return a;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_let_list_destructuring_to_index_reads() {
    let function = compile_source(
        r#"
        let [a, _, [b]] = [40, 99, [2]];
        return a + b;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_let_list_rest_destructuring_to_slice_from() {
    let function = compile_source(
        r#"
        let [head, ..tail] = [40, 1, 2];
        return head + tail.1;
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
fn compiler_lowers_let_map_destructuring_to_index_reads() {
    let function = compile_source(
        r#"
        let {"left": a, "right": {"value": b}} = {"left": 40, "right": {"value": 2}};
        return a + b;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_lowers_let_map_rest_destructuring_to_map_rest() {
    let function = compile_source(
        r#"
        let {"a": a, ..rest} = {"a": 40, "b": 2};
        return a + rest.b;
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
fn compiler_lowers_program_expression_to_nil_without_return() {
    let function = compile_source(
        r#"
        let ignored = 1 + 2;
        ignored;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Nil]);
}

#[test]
fn compiler_elides_nil_load_for_empty_return_paths() {
    for source in [
        "return;",
        r#"
        let ignored = 1 + 2;
        ignored;
        "#,
    ] {
        let function = compile_source(source).expect("compile source");
        assert!(
            function.code.iter().all(|instr| instr.opcode() != Opcode::LoadNil),
            "empty return should not materialize nil: {:?}",
            function.code
        );

        let return_instr = function
            .code
            .iter()
            .find(|instr| instr.opcode().is_return())
            .expect("Return");
        assert_eq!(return_instr.return_count(), 0, "empty return should use Return count=0");

        let result = execute(&function).expect("execute");
        assert_eq!(result.returns, vec![crate::val::RuntimeVal::Nil]);
    }
}

#[test]
fn compiler_lowers_if_else_assignment() {
    let function = compile_source(
        r#"
        let x = 1;
        if (true) {
            x = 10;
        } else {
            x = 20;
        }
        return x;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler_lowers_conditional_expression() {
    let function = compile_source(
        r#"
        let x = false ? 10 : 20;
        return x;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(20)]);
}

#[test]
fn compiler_lowers_int_math_floor_directly_into_destination() {
    let program = parse_program(
        r#"
        let lo = 2;
        let hi = 8;
        let mid = math.floor((lo + hi) / 2);
        return mid;
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["math"]).expect("compile module");
    let function = module.entry_function().expect("entry function");

    let mid = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::MidInt)
        .expect("expected midpoint opcode for floor argument");
    let mid_return = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::Return1)
        .expect("expected single return");

    assert_eq!(
        mid.a(),
        mid_return.a(),
        "math.floor(Int midpoint) should write directly into the destination local: {:?}",
        function.code
    );
    assert!(
        !function
            .code
            .windows(2)
            .any(|window| window[0].opcode() == Opcode::DivInt && window[1].opcode() == Opcode::Move),
        "math.floor(Int) should not emit DivInt followed by a destination Move: {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::DivInt),
        "midpoint floor should avoid AddInt + DivInt arithmetic chain: {:?}",
        function.code
    );
}

#[test]
fn compiler_midpoint_floor_preserves_current_int_division_semantics() {
    let program = parse_program(
        r#"
        let lo = -5;
        let hi = 2;
        return math.floor((lo + hi) / 2);
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["math"]).expect("compile module");
    let function = module.entry_function().expect("entry function");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::MidInt),
        "midpoint floor should lower to MidInt: {:?}",
        function.code
    );
    let result = crate::vm::exec::execute_compiled_module_with_ctx(
        alloc::sync::Arc::new(module),
        &mut crate::vm::VmContext::new(),
    )
    .expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(-1)]);
}

#[test]
fn compiler_lowers_map_get_directly_into_destination() {
    let program = parse_program(
        r#"
        let values = {"x": 42};
        let key = "x";
        let value = map.get(values, key);
        return value;
        "#,
    );
    let module =
        Compiler::compile_module_with_natives_and_globals(&program, Vec::new(), ["map"]).expect("compile module");
    let function = module.entry_function().expect("entry function");

    let get = function
        .code
        .iter()
        .find(|instr| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetFieldK))
        .expect("expected map.get lowering");
    let value_return = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::Return1)
        .expect("expected single return");

    assert_eq!(
        get.a(),
        value_return.a(),
        "map.get should write directly into the destination local: {:?}",
        function.code
    );
    assert!(
        !function.code.windows(2).any(
            |window| matches!(window[0].opcode(), Opcode::GetIndex | Opcode::GetFieldK)
                && window[1].opcode() == Opcode::Move
        ),
        "map.get should not emit GetIndex/GetFieldK followed by a destination Move: {:?}",
        function.code
    );
}

#[test]
fn compiler_lowers_access_directly_into_destination() {
    let function = compile_source(
        r#"
        let values = [10, 20, 30];
        let index = 1;
        let item = values[index];
        let settings = {"mode": 7};
        let mode = settings["mode"];
        return item + mode;
        "#,
    )
    .expect("compile source");

    let item_get = function
        .code
        .iter()
        .find(|instr| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetList) && instr.a() == 2)
        .expect("expected list access to write into item local");
    assert_eq!(item_get.a(), 2);

    let mode_get = function
        .code
        .iter()
        .find(|instr| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetFieldK) && instr.a() == 4)
        .expect("expected map access to write into mode local");
    assert_eq!(mode_get.a(), 4);

    assert!(
        !function.code.windows(2).any(|window| matches!(
            window[0].opcode(),
            Opcode::GetIndex | Opcode::GetList | Opcode::GetFieldK
        ) && window[1].opcode() == Opcode::Move),
        "access lowering should not emit GetIndex/GetList/GetFieldK followed by destination Move: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(27)]);
}

#[test]
fn compiler_lowers_list_literal_to_heap_list() {
    let function = compile_source("return [1, 2 + 3, \"x\"];").expect("compile source");

    let result = execute(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let crate::val::HeapValue::List(crate::val::TypedList::Mixed(values)) =
        result.state.heap.get(handle).expect("heap object")
    else {
        panic!("expected mixed list");
    };

    assert_eq!(values[0], crate::val::RuntimeVal::Int(1));
    assert_eq!(values[1], crate::val::RuntimeVal::Int(5));
    assert_eq!(values[2].kind(), crate::val::RuntimeValKind::ShortStr);
}

#[test]
fn compiler_lowers_container_elements_directly_into_build_window() {
    let function = compile_source(
        r#"
        let a = 1;
        let b = 2;
        return [a + 1, b + 2];
        "#,
    )
    .expect("compile source");

    let source_move_facts = function
        .code
        .iter()
        .enumerate()
        .filter(|(_, instr)| instr.opcode() == Opcode::Move)
        .filter_map(|(pc, _)| function.performance.register_copy(pc))
        .filter(|fact| fact.move_source)
        .collect::<Vec<_>>();

    assert!(
        source_move_facts.is_empty(),
        "expected direct list window lowering without source materialization moves in {:?}",
        function.code
    );
    let new_list_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::NewList)
        .expect("new list instruction");
    let build = function
        .performance
        .container_build(new_list_pc)
        .expect("container build fact");
    assert!(build.move_values);

    let result = execute(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected list object");
    };
    let crate::val::HeapValue::List(crate::val::TypedList::Int(values)) =
        result.state.heap.get(handle).expect("heap object")
    else {
        panic!("expected typed int list");
    };
    assert_eq!(values, &vec![2, 4]);
}

#[test]
fn compiler_records_register_performance_facts_for_literals_and_containers() {
    let list_expr = Expr::List(vec![
        Box::new(Expr::Literal(LiteralVal::Int(1))),
        Box::new(Expr::Literal(LiteralVal::Int(2))),
    ]);
    let list_function = compile_expr(&list_expr).expect("compile list");
    let list_reg = list_function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::LoadHeapConst || instr.opcode() == Opcode::NewList)
        .expect("list-producing instruction")
        .a() as u16;
    assert_eq!(
        list_function.performance.value_kind(list_reg),
        crate::vm::analysis::PerfValueKind::List
    );
    assert_eq!(
        list_function.performance.list_value_kind(list_reg),
        Some(crate::vm::analysis::PerfValueKind::Int)
    );
    assert_eq!(list_function.performance.list_known_len(list_reg), Some(2));

    let map_expr = Expr::Map(vec![(
        Box::new(Expr::Literal(LiteralVal::from_str("answer"))),
        Box::new(Expr::Literal(LiteralVal::Int(42))),
    )]);
    let map_function = compile_expr(&map_expr).expect("compile map");
    let map_reg = map_function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::LoadHeapConst || instr.opcode() == Opcode::NewMap)
        .expect("map-producing instruction")
        .a() as u16;
    assert_eq!(
        map_function.performance.value_kind(map_reg),
        crate::vm::analysis::PerfValueKind::Map
    );
    assert_eq!(
        map_function.performance.map_value_kind(map_reg),
        Some(crate::vm::analysis::PerfValueKind::Int)
    );
}

#[test]
fn compiler_lowers_range_expression_to_typed_int_list() {
    let function = compile_source("return 5..=1..0 - 2;").expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::NewRange),
        "expected NewRange in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    let crate::val::RuntimeVal::Obj(handle) = result.returns[0] else {
        panic!("expected range list object");
    };
    let crate::val::HeapValue::List(crate::val::TypedList::Int(values)) =
        result.state.heap.get(handle).expect("heap object")
    else {
        panic!("expected typed int list");
    };

    assert_eq!(values, &vec![5, 3, 1]);
}

#[test]
fn compiler_lowers_map_literal_and_string_access() {
    let function = compile_source(r#"return {"answer": 42}.answer;"#).expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

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

#[test]
fn compiler_trait_method_dispatch_uses_runtime_callable() {
    let program = parse_program(
        r#"
        struct Rect { w: Int, h: Int }
        fn area(self) {
            return self.w * self.h;
        }
        __lk_register_trait("Area", [["area", "Function"]]);
        __lk_register_trait_impl("Area", "Rect", [["area", area, nil]]);
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
