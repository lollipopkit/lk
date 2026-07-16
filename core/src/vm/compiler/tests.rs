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
mod misc;
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
