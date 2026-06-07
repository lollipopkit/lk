use super::*;
use crate::{
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    val::RuntimeVal,
    vm::analysis::{PerfCallTargetKind, PerfIndexTargetKind, PerfValueKind},
    vm::{NativeArgs, NativeEntry, NativeFunction, NativeRuntime, Opcode, execute, execute_module},
};
use anyhow::{Result, bail};

fn compile_source(source: &str) -> Function {
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let program = StmtParser::new(&tokens).parse_program().expect("parse");
    compile_program(&program).expect("compile")
}

#[test]
fn compiler_records_container_move_fact_for_rewritten_set_index() {
    let function = compile_source(
        r#"
        let xs = [1, 2, 3];
        xs = list.set(xs, 1, 40 + 2).0;
        return xs.1;
        "#,
    );

    let set_index_pc = function
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK))
        .expect("set index opcode");
    let fact = function
        .performance
        .container_move(set_index_pc)
        .expect("container move fact");
    assert!(fact.move_key);
    assert!(fact.move_value);

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_records_container_build_fact_for_list_literal() {
    let function = compile_source(
        r#"
        let x = 1 + 2;
        return [x, x + 4];
        "#,
    );
    let new_list_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::NewList)
        .expect("NewList");
    let fact = function
        .performance
        .container_build(new_list_pc)
        .expect("container build fact");

    assert!(!fact.move_keys);
    assert!(fact.move_values);

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(_) = result.returns[0] else {
        panic!("expected returned list object");
    };
}

#[test]
fn compiler_records_container_build_fact_for_map_literal() {
    let function = compile_source(
        r#"
        let x = 1 + 2;
        return {"a": x, "b": x + 4};
        "#,
    );
    let new_map_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::NewMap)
        .expect("NewMap");
    let fact = function
        .performance
        .container_build(new_map_pc)
        .expect("container build fact");

    assert!(fact.move_keys);
    assert!(fact.move_values);

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(_) = result.returns[0] else {
        panic!("expected returned map object");
    };
}

#[test]
fn compiler_keeps_container_literal_source_locals_readable() {
    let list_function = compile_source(
        r#"
        let x = "source-long-string";
        let xs = [x];
        return x;
        "#,
    );
    let list_result = execute(&list_function).expect("execute list source");
    let RuntimeVal::Obj(_) = list_result.returns[0] else {
        panic!("expected list source local to remain readable");
    };

    let map_function = compile_source(
        r#"
        let key = "answer";
        let value = "source-long-string";
        let values = {key: value};
        return value;
        "#,
    );
    let map_result = execute(&map_function).expect("execute map source");
    let RuntimeVal::Obj(_) = map_result.returns[0] else {
        panic!("expected map value local to remain readable");
    };
}

#[test]
fn executor_moves_returned_value_from_stack_window() {
    let function = compile_source("return [1, 2, 3];");

    let result = execute(&function).expect("execute");
    let RuntimeVal::Obj(_) = result.returns[0] else {
        panic!("expected returned list object");
    };
    assert!(
        result
            .state
            .stack
            .iter()
            .all(|value| !matches!(value, RuntimeVal::Obj(_))),
        "return move should consume the source register from the executor stack"
    );
}

#[test]
fn compiler_keeps_local_set_index_key_readable() {
    let function = compile_source(
        r#"
        let scores = {"a": 1};
        let key = "b";
        scores[key] = 40;
        return key + ":" + scores.b;
        "#,
    );

    let set_index_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::SetIndex)
        .expect("SetIndex");
    let fact = function
        .performance
        .container_move(set_index_pc)
        .expect("container move fact");
    assert!(!fact.move_key);
    assert!(fact.move_value);

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns.len(), 1);
    assert_eq!(result.returns[0].kind(), crate::val::RuntimeValKind::ShortStr);
}

#[test]
fn compiler_reuses_local_set_index_target_and_value() {
    let function = compile_source(
        r#"
        let scores = {"a": "old-long-string"};
        let value = "new-long-string";
        scores["a"] = value;
        return value;
        "#,
    );

    let set_pc = function
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK))
        .expect("set index opcode");
    let instr = function.code[set_pc];
    let fact = function
        .performance
        .container_move(set_pc)
        .expect("container move fact");

    assert!(
        function.performance.is_local_slot(instr.a() as u16),
        "set opcode should mutate the local target directly"
    );
    assert!(
        function.performance.is_local_slot(instr.b() as u16),
        "SetFieldK should read the local value directly"
    );
    assert!(!fact.move_value, "set opcode must keep current local values readable");

    let result = execute(&function).expect("execute");
    assert!(matches!(result.returns.as_slice(), [RuntimeVal::Obj(_)]));
}

#[test]
fn compiler_records_set_index_target_shape_facts() {
    let function = compile_source(
        r#"
        let values = [1, 2, 3];
        values[1] = 4;
        let scores = {"a": 1};
        scores["b"] = 40;
        let user = User { score: 1 };
        user.score = 2;
        return values.1 + scores.b + user.score;
        "#,
    );

    let index_facts = function
        .code
        .iter()
        .enumerate()
        .filter(|(_, instr)| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK))
        .filter_map(|(pc, _)| function.performance.index_op(pc).copied())
        .collect::<Vec<_>>();

    assert!(
        index_facts
            .iter()
            .any(|fact| fact.target_kind == PerfIndexTargetKind::List && fact.value_kind == PerfValueKind::Int)
    );
    assert!(
        index_facts
            .iter()
            .any(|fact| fact.target_kind == PerfIndexTargetKind::Map && fact.value_kind == PerfValueKind::Int)
    );
    assert!(
        index_facts
            .iter()
            .any(|fact| fact.target_kind == PerfIndexTargetKind::Object)
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(46)]);
}

#[test]
fn compiler_records_short_string_key_fact_for_set_index() {
    let function = compile_source(
        r#"
        let scores = {"a": 1};
        scores["b"] = 2;
        let user = User { score: 1 };
        user.score = 40;
        return scores.b + user.score;
        "#,
    );

    let set_index_keys = function
        .code
        .iter()
        .enumerate()
        .filter(|(_, instr)| matches!(instr.opcode(), Opcode::SetIndex | Opcode::SetFieldK))
        .filter_map(|(pc, _)| {
            let instr = function.code[pc];
            match instr.opcode() {
                Opcode::SetFieldK => function.consts.string(instr.c() as u16),
                Opcode::SetIndex => function
                    .performance
                    .known_key(pc)
                    .and_then(|fact| fact.const_key)
                    .and_then(|key| function.consts.string(key)),
                _ => None,
            }
        })
        .collect::<Vec<_>>();

    assert!(set_index_keys.contains(&"b"));
    assert!(set_index_keys.contains(&"score"));

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_elides_known_map_and_object_set_key_materialization() {
    let function = compile_source(
        r#"
        let scores = {"a": 1};
        scores["b"] = 2;
        let user = User { score: 1 };
        user.score = 40;
        return scores.b + user.score;
        "#,
    );

    let set_fields = function
        .code
        .iter()
        .enumerate()
        .filter(|(_, instr)| instr.opcode() == Opcode::SetFieldK)
        .collect::<Vec<_>>();

    assert_eq!(set_fields.len(), 2, "expected map and object SetFieldK");
    for (pc, instr) in set_fields {
        assert_eq!(
            function.consts.string(instr.c() as u16).is_some(),
            true,
            "SetFieldK should carry the const key index inline"
        );
        assert!(
            pc.checked_sub(1)
                .and_then(|prev| function.code.get(prev))
                .is_none_or(|prev| prev.opcode() != Opcode::LoadString),
            "known Map/Object string key should not materialize a LoadString immediately before SetFieldK"
        );
    }

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn set_index_shape_facts_preserve_typed_backing_on_matching_writes() {
    let list_function = compile_source(
        r#"
        let values = [1, 2, 3];
        values[1] = 4;
        return values;
        "#,
    );
    let list_result = execute(&list_function).expect("execute list");
    let RuntimeVal::Obj(list_handle) = list_result.returns[0] else {
        panic!("expected list object");
    };
    let crate::val::HeapValue::List(crate::val::TypedList::Int(values)) =
        list_result.state.heap.get(list_handle).expect("list heap object")
    else {
        panic!("expected typed int list");
    };
    assert_eq!(values, &vec![1, 4, 3]);

    let map_function = compile_source(
        r#"
        let scores = {"a": 1};
        scores["b"] = 2;
        return scores;
        "#,
    );
    let map_result = execute(&map_function).expect("execute map");
    let RuntimeVal::Obj(map_handle) = map_result.returns[0] else {
        panic!("expected map object");
    };
    let crate::val::HeapValue::Map(crate::val::TypedMap::StringInt(values)) =
        map_result.state.heap.get(map_handle).expect("map heap object")
    else {
        panic!("expected typed string-int map");
    };
    assert_eq!(values.get("a"), Some(&1));
    assert_eq!(values.get("b"), Some(&2));
}

#[test]
fn compiler_marks_pure_literal_expression_statement_as_dead_write() {
    let function = compile_source(
        r#"
        123;
        "short";
        return 42;
        "#,
    );

    let dead_loads = function
        .code
        .iter()
        .enumerate()
        .filter(|(pc, instr)| {
            matches!(instr.opcode(), Opcode::LoadInt | Opcode::LoadString) && function.performance.dead_write(*pc)
        })
        .count();
    assert_eq!(dead_loads, 2);

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_does_not_copy_discarded_local_expression_statement() {
    let function = compile_source(
        r#"
        let payload = "source-long-string";
        payload;
        return payload;
        "#,
    );

    let moves = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::Move)
        .count();
    assert_eq!(moves, 0, "discarded local expression should not copy into a temporary");

    let result = execute(&function).expect("execute");
    assert!(matches!(result.returns.as_slice(), [RuntimeVal::Obj(_)]));
}

#[test]
fn compiler_does_not_copy_discarded_wildcard_local_binding() {
    let function = compile_source(
        r#"
        let payload = "source-long-string";
        let _ = payload;
        return payload;
        "#,
    );

    let moves = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::Move)
        .count();
    assert_eq!(
        moves, 0,
        "wildcard binding from a current local should not copy into a temporary"
    );

    let result = execute(&function).expect("execute");
    assert!(matches!(result.returns.as_slice(), [RuntimeVal::Obj(_)]));
}

#[test]
fn compiler_does_not_copy_returned_local() {
    let function = compile_source(
        r#"
        let payload = "source-long-string";
        return payload;
        "#,
    );

    let moves = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::Move)
        .count();
    assert_eq!(moves, 0, "returning a current local should not copy into a temporary");

    let return_instr = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::Return)
        .expect("Return");
    assert!(
        function.performance.is_local_slot(return_instr.a() as u16),
        "Return should consume the current local slot directly"
    );

    let result = execute(&function).expect("execute");
    assert!(matches!(result.returns.as_slice(), [RuntimeVal::Obj(_)]));
}

#[test]
fn compiler_keeps_direct_if_let_binding_isolated_from_scrutinee() {
    let function = compile_source(
        r#"
        let payload = 1;
        if let x = payload {
            x = 2;
        }
        return payload;
        "#,
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![RuntimeVal::Int(1)]);
}

#[test]
fn compiler_reuses_local_if_let_list_scrutinee() {
    let function = compile_source(
        r#"
        let payload = ["source-long-string"];
        if let [item] = payload {
            return item;
        }
        return nil;
        "#,
    );

    let is_list = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::IsList)
        .expect("IsList");
    assert!(
        function.performance.is_local_slot(is_list.b() as u16),
        "if-let list shape check should read the local scrutinee directly"
    );

    let result = execute(&function).expect("execute");
    assert!(matches!(result.returns.as_slice(), [RuntimeVal::Obj(_)]));
}

#[test]
fn compiler_reuses_local_range_pattern_bound() {
    let function = compile_source(
        r#"
        let end = 65;
        let age = 42;
        if let 18..end = age {
            return age;
        }
        return 0;
        "#,
    );

    let upper_cmp = function
        .code
        .iter()
        .find(|instr| matches!(instr.opcode(), Opcode::CmpLtInt | Opcode::TestLtInt))
        .expect("upper range comparison");
    let (lhs, rhs) = if upper_cmp.opcode().is_compare_test() {
        (upper_cmp.a(), upper_cmp.b())
    } else {
        (upper_cmp.b(), upper_cmp.c())
    };
    assert!(
        function.performance.is_local_slot(lhs as u16),
        "range pattern should compare the local scrutinee directly"
    );
    assert!(
        function.performance.is_local_slot(rhs as u16),
        "range pattern should compare against the local upper bound directly"
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![RuntimeVal::Int(42)]);
}

#[test]
fn compiler_moves_map_rest_temp_keys_but_keeps_local_source() {
    let function = compile_source(
        r#"
        let data = {"a": "source-long-string", "b": 2};
        let {"a": a, ..rest} = data;
        return data.a;
        "#,
    );

    let (map_rest_pc, map_rest) = function
        .code
        .iter()
        .enumerate()
        .find(|(_, instr)| instr.opcode() == Opcode::MapRest)
        .expect("MapRest");
    let base = map_rest.b();
    let key_count = map_rest.c();

    let source_move_pc = function.code[..map_rest_pc]
        .iter()
        .enumerate()
        .rfind(|(_, instr)| instr.opcode() == Opcode::Move && instr.a() == base)
        .map(|(pc, _)| pc)
        .expect("map rest source move");
    let source_fact = function
        .performance
        .register_copy(source_move_pc)
        .expect("source move fact");
    assert!(
        !source_fact.move_source,
        "MapRest must keep a current local source map readable"
    );

    for offset in 0..key_count {
        let key_dst = base + 1 + offset;
        let key_move_pc = function.code[..map_rest_pc]
            .iter()
            .enumerate()
            .rfind(|(_, instr)| instr.opcode() == Opcode::Move && instr.a() == key_dst)
            .map(|(pc, _)| pc)
            .expect("map rest key move");
        let key_fact = function.performance.register_copy(key_move_pc).expect("key move fact");
        assert!(key_fact.move_source, "MapRest temp key registers can be consumed");
    }

    let result = execute(&function).expect("execute");
    assert!(matches!(result.returns.as_slice(), [RuntimeVal::Obj(_)]));
}

#[test]
fn compiler_does_not_mark_heap_literal_expression_statement_as_dead_write() {
    let function = compile_source(
        r#"
        "longer-than-seven";
        return 42;
        "#,
    );

    let heap_const_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::LoadHeapConst)
        .expect("heap const load");
    assert!(!function.performance.dead_write(heap_const_pc));

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_records_cell_move_fact_for_upvalue_store_sources() {
    let module = compile_source_module(
        r#"
        fn make() {
            let value = "stored-long-string";
            let set = || {
                value = "next-long-string";
                return value;
            };
            return set();
        }

        return make();
        "#,
    )
    .expect("compile module");

    let store_cell_facts = module
        .functions
        .iter()
        .flat_map(|function| {
            function
                .code
                .iter()
                .enumerate()
                .filter(|(_, instr)| instr.opcode() == Opcode::StoreCellVal)
                .map(|(pc, _)| function.performance.cell_move(pc).copied())
        })
        .collect::<Vec<_>>();

    assert!(!store_cell_facts.is_empty(), "expected StoreCellVal instructions");
    assert!(
        store_cell_facts
            .iter()
            .all(|fact| fact.is_some_and(|fact| fact.move_value)),
        "compiler-generated StoreCellVal should consume dead source temporaries"
    );
}

#[test]
fn compiler_reuses_current_local_for_cell_assignment() {
    let module = compile_source_module(
        r#"
        fn make() {
            let value = "stored-long-string";
            let replacement = "replacement-long-string";
            let get = || {
                return value;
            };
            value = replacement;
            return replacement;
        }

        return make();
        "#,
    )
    .expect("compile module");

    let local_cell_stores = module
        .functions
        .iter()
        .flat_map(|function| {
            function
                .code
                .iter()
                .enumerate()
                .filter(|(_, instr)| instr.opcode() == Opcode::StoreCellVal)
                .filter_map(move |(pc, instr)| {
                    function
                        .performance
                        .cell_move(pc)
                        .map(|fact| (function, pc, *instr, fact))
                })
        })
        .filter(|(function, _, instr, fact)| !fact.move_value && function.performance.is_local_slot(instr.b() as u16))
        .collect::<Vec<_>>();

    assert!(
        !local_cell_stores.is_empty(),
        "assigning a current local into a cell should copy from that local"
    );
    assert!(
        local_cell_stores.iter().all(|(function, pc, instr, _)| {
            pc.checked_sub(1)
                .and_then(|prev| function.code.get(prev))
                .is_none_or(|prev| prev.opcode() != Opcode::Move || prev.a() != instr.b())
        }),
        "cell assignment from a current local should not need an intermediate Move"
    );

    let result = execute_module(&module).expect("execute module");
    assert!(matches!(result.returns.as_slice(), [RuntimeVal::Obj(_)]));
}

#[test]
fn compiler_reuses_current_local_for_global_assignment() {
    let module = compile_source_module(
        r#"
        target := "initial-long-string";

        fn update() {
            let value = "replacement-long-string";
            target = value;
            return value;
        }

        return update();
        "#,
    )
    .expect("compile module");
    let target_slot = module
        .globals
        .iter()
        .position(|slot| slot.name.as_ref() == "target")
        .expect("target global") as u16;
    let target_sets = module
        .functions
        .iter()
        .flat_map(|function| {
            function
                .code
                .iter()
                .enumerate()
                .filter(|(_, instr)| instr.opcode() == Opcode::SetGlobal)
                .filter_map(move |(pc, _)| function.performance.global_op(pc).map(|fact| (function, pc, fact)))
        })
        .filter(|(_, _, fact)| fact.slot == target_slot)
        .collect::<Vec<_>>();

    assert_eq!(target_sets.len(), 2, "expected the global initializer and assignment");
    assert!(
        target_sets.iter().all(|(_, _, fact)| !fact.move_source),
        "global assignment from a current local should copy instead of taking it"
    );
    assert!(
        target_sets.iter().all(|(function, pc, _)| {
            pc.checked_sub(1)
                .and_then(|prev| function.code.get(prev))
                .is_none_or(|instr| instr.opcode() != Opcode::Move)
        }),
        "global assignment from a current local should not need an intermediate Move"
    );

    let result = execute_module(&module).expect("execute module");
    assert!(matches!(result.returns.as_slice(), [RuntimeVal::Obj(_)]));
}

#[test]
fn compiler_records_short_string_key_fact_for_get_index() {
    let function = compile_source(r#"return {"answer": 42}.answer;"#);

    let get_field = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::GetFieldK)
        .expect("GetFieldK");
    assert_eq!(function.consts.string(get_field.c() as u16), Some("answer"));

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_elides_known_map_and_object_get_key_materialization() {
    let function = compile_source(
        r#"
        let scores = {"answer": 40};
        let user = User { score: 2 };
        return scores.answer + user.score;
        "#,
    );

    let get_fields = function
        .code
        .iter()
        .enumerate()
        .filter(|(_, instr)| instr.opcode() == Opcode::GetFieldK)
        .collect::<Vec<_>>();

    assert_eq!(get_fields.len(), 2, "expected map and object GetFieldK");
    for (pc, instr) in get_fields {
        assert!(
            function.consts.string(instr.c() as u16).is_some(),
            "GetFieldK should carry the const key index inline"
        );
        assert!(
            pc.checked_sub(1)
                .and_then(|prev| function.code.get(prev))
                .is_none_or(|prev| prev.opcode() != Opcode::LoadString),
            "known Map/Object string key should not materialize a LoadString immediately before GetFieldK"
        );
    }

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_does_not_record_long_string_key_fact_for_get_index() {
    let function = compile_source(r#"return {"longer-than-seven": 42}."longer-than-seven";"#);

    let get_index_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::GetIndex)
        .expect("GetIndex");
    assert!(function.performance.known_key(get_index_pc).is_none());

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler_records_index_target_shape_facts() {
    let function = compile_source(
        r#"
        let list_value = [40, 42].1;
        let map_value = {"answer": 42}.answer;
        let string_value = "az".1;
        let user = User { score: 42 };
        let object_value = user.score;
        return list_value + map_value + object_value;
        "#,
    );

    let index_facts = function
        .code
        .iter()
        .enumerate()
        .filter(|(_, instr)| matches!(instr.opcode(), Opcode::GetIndex | Opcode::GetFieldK))
        .filter_map(|(pc, _)| function.performance.index_op(pc).copied())
        .collect::<Vec<_>>();

    assert!(
        index_facts
            .iter()
            .any(|fact| fact.target_kind == PerfIndexTargetKind::List && fact.value_kind == PerfValueKind::Int)
    );
    assert!(
        index_facts
            .iter()
            .any(|fact| fact.target_kind == PerfIndexTargetKind::Map && fact.value_kind == PerfValueKind::Int)
    );
    assert!(
        index_facts
            .iter()
            .any(|fact| fact.target_kind == PerfIndexTargetKind::String)
    );
    assert!(
        index_facts
            .iter()
            .any(|fact| fact.target_kind == PerfIndexTargetKind::Object)
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(126)]);
}

#[test]
fn compiler_records_control_flow_facts_after_jump_patching() {
    let function = compile_source(
        r#"
        let value = 0;
        if true {
            value = 1;
        } else {
            value = 2;
        }
        return value;
        "#,
    );

    let branch_pc = function
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode::Test | Opcode::BrFalse | Opcode::BrTrue))
        .expect("conditional branch");
    let jmp_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::Jmp)
        .expect("Jmp");
    let branch = function.code[branch_pc];
    let jmp = function.code[jmp_pc];
    let branch_taken = match branch.opcode() {
        Opcode::Test => ((branch_pc as i64) + 1 + i64::from(branch.c() as i8)) as usize,
        Opcode::BrFalse | Opcode::BrTrue => ((branch_pc as i64) + 1 + i64::from(branch.sbx())) as usize,
        _ => unreachable!(),
    };
    let jmp_target = ((jmp_pc as i64) + 1 + i64::from(jmp.sj_arg())) as usize;

    if branch.opcode() == Opcode::Test {
        assert!(function.performance.is_branch_target(branch_pc + 1));
    }
    assert!(function.performance.is_branch_target(branch_taken));
    assert!(function.performance.is_branch_target(jmp_target));
    assert!(!function.performance.same_block(branch_pc, branch_pc + 1));
    assert!(!function.performance.same_block(branch_pc + 1, branch_taken));

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(1)]);
}

#[test]
fn compiler_records_loop_backedge_as_branch_target() {
    let function = compile_source(
        r#"
        let value = 0;
        while (value < 3) {
            value = value + 1;
        }
        return value;
        "#,
    );

    let loop_backedge = function
        .code
        .iter()
        .enumerate()
        .find(|(_, instr)| instr.opcode() == Opcode::Jmp && instr.sj_arg() < 0)
        .expect("loop backedge");
    let cmp_pc = function
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode::CmpLtInt | Opcode::TestLtInt))
        .expect("loop compare");
    let target = ((loop_backedge.0 as i64) + 1 + i64::from(loop_backedge.1.sj_arg())) as usize;

    assert!(function.performance.is_branch_target(target));
    assert!(!function.performance.same_block(loop_backedge.0, target));
    if function.code[cmp_pc].opcode() == Opcode::CmpLtInt {
        let fused = function
            .performance
            .fused_bool_branch(cmp_pc)
            .expect("compare branch fusion fact");
        assert_eq!(fused.result_reg, function.code[cmp_pc].a());
    }

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler_records_positional_call_shape_fact() {
    let module = compile_source_module(
        r#"
        fn add(a, b) {
            return a + b;
        }
        return add(40, 2);
        "#,
    )
    .expect("compile module");
    let function = &module.functions[0];
    let call_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::CallDirect)
        .expect("CallDirect");
    let fact = function.performance.call_site(call_pc).expect("call fact");

    assert_eq!(fact.call_base, function.code[call_pc].a() as u16);
    assert_eq!(fact.positional_count, 2);
    assert_eq!(fact.named_count, 0);
    assert_eq!(fact.target_kind, PerfCallTargetKind::Closure);
}

#[test]
fn compiler_direct_module_call_does_not_copy_callable_into_window() {
    let module = compile_source_module(
        r#"
        fn id(x) {
            return x;
        }
        return id(42);
        "#,
    )
    .expect("compile module");
    let function = &module.functions[0];
    let call_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::CallDirect)
        .expect("CallDirect");
    let call_base = function.code[call_pc].a();

    assert!(
        !function.code[..call_pc]
            .iter()
            .any(|instr| instr.opcode() == Opcode::Move && instr.a() == call_base),
        "direct module function call should not copy a heap callable into the return slot"
    );
}

#[test]
fn compiler_lowers_direct_call_expression_args_into_call_window() {
    let module = compile_source_module(
        r#"
        fn add(a, b) {
            return a + b;
        }
        return add(40 + 1, 1 + 0);
        "#,
    )
    .expect("compile module");
    let function = &module.functions[0];
    let call_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::CallDirect)
        .expect("CallDirect");
    let call_base = function.code[call_pc].a();
    let arg0 = call_base + 1;
    let arg1 = call_base + 2;

    assert!(
        !function.code[..call_pc]
            .iter()
            .any(|instr| instr.opcode() == Opcode::Move && (instr.a() == arg0 || instr.a() == arg1)),
        "call expression arguments should lower directly into the call window"
    );
}

#[test]
fn compiler_lowers_named_signature_args_into_direct_call_window() {
    let module = compile_source_module(
        r#"
        fn add(a, {b: Int? = a + 1}) {
            return a + b;
        }
        return add(20 + 0);
        "#,
    )
    .expect("compile module");
    let function = &module.functions[0];
    let call_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::CallDirect)
        .expect("CallDirect");
    let call_base = function.code[call_pc].a() as u16;
    let arg0 = call_base + 1;
    let arg1 = call_base + 2;

    assert!(
        !function.code[..call_pc]
            .iter()
            .any(|instr| instr.opcode() == Opcode::Move && (instr.a() as u16 == arg0 || instr.a() as u16 == arg1)),
        "named-signature direct call arguments should lower directly into the call window"
    );
}

#[test]
fn compiler_records_dynamic_named_call_shape_fact() {
    let module = compile_source_module(
        r#"
        let make = || |x| x;
        return make()(40 + 1, x: 1 + 0);
        "#,
    )
    .expect("compile module");
    let function = &module.functions[0];
    let call_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::CallNamed)
        .expect("CallNamed");
    let fact = function.performance.call_site(call_pc).expect("call fact");

    assert_eq!(fact.call_base, function.code[call_pc].a() as u16);
    assert_eq!(fact.positional_count, 1);
    assert_eq!(fact.named_count, 1);
    assert_eq!(fact.target_kind, PerfCallTargetKind::Unknown);
    let callee_move_pc = function.code[..call_pc]
        .iter()
        .rposition(|instr| instr.opcode() == Opcode::Move && instr.a() as u16 == fact.call_base)
        .expect("callee move");
    assert!(
        function
            .performance
            .register_copy(callee_move_pc)
            .is_some_and(|fact| fact.move_source),
        "temporary dynamic callee should move into the call window"
    );
    let first_arg = fact.call_base + 1;
    let named_key = fact.call_base + 2;
    let named_value = fact.call_base + 3;
    assert!(
        !function.code[..call_pc]
            .iter()
            .any(|instr| instr.opcode() == Opcode::Move
                && matches!(instr.a() as u16, dst if dst == first_arg || dst == named_key || dst == named_value)),
        "dynamic named call arguments should lower directly into the call window"
    );
}

#[test]
fn compiler_records_native_call_target_shape_fact() {
    fn native_id(args: NativeArgs<'_>, _runtime: &mut NativeRuntime<'_>) -> Result<RuntimeVal> {
        let [RuntimeVal::Int(value)] = args.as_slice() else {
            bail!("native_id expects one int");
        };
        Ok(RuntimeVal::Int(*value))
    }

    let module = compile_source_module_with_natives(
        "return native_id(42);",
        vec![NativeEntry {
            name: "native_id".to_string(),
            arity: 1,
            function: NativeFunction::Plain(native_id),
        }],
    )
    .expect("compile module");
    let function = &module.functions[0];
    let call_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::Call)
        .expect("Call");
    let fact = function.performance.call_site(call_pc).expect("call fact");

    assert_eq!(fact.target_kind, PerfCallTargetKind::Native);
}

#[test]
fn compiler_records_global_slot_facts_for_get_and_set() {
    let module = compile_source_module(
        r#"
        counter := 40;
        fn bump() {
            counter = counter + 2;
            return counter;
        }
        return bump();
        "#,
    )
    .expect("compile module");
    let global_facts = module
        .functions
        .iter()
        .flat_map(|function| {
            function
                .code
                .iter()
                .enumerate()
                .filter(|(_, instr)| matches!(instr.opcode(), Opcode::GetGlobal | Opcode::SetGlobal))
                .map(|(pc, instr)| (instr.opcode(), function.performance.global_op(pc).expect("global fact")))
        })
        .collect::<Vec<_>>();

    assert!(global_facts.iter().any(|(opcode, fact)| *opcode == Opcode::GetGlobal
        && module.globals[fact.slot as usize].name.as_ref() == "counter"
        && !fact.move_source));
    assert!(global_facts.iter().any(|(opcode, fact)| *opcode == Opcode::SetGlobal
        && module.globals[fact.slot as usize].name.as_ref() == "counter"
        && fact.move_source));
}
