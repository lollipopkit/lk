use super::*;
use crate::{
    stmt::stmt_parser::StmtParser,
    token::Tokenizer,
    val::RuntimeVal,
    vm::analysis::{PerfCallTargetKind, PerfIndexTargetKind, PerfValueKind},
    vm::{NativeArgs32, NativeEntry32, NativeFunction32, NativeRuntime32, Opcode32, execute32},
};
use anyhow::{Result, bail};

fn compile_source(source: &str) -> Function32 {
    let tokens = Tokenizer::tokenize(source).expect("tokenize");
    let program = StmtParser::new(&tokens).parse_program().expect("parse");
    compile_program32(&program).expect("compile")
}

#[test]
fn compiler32_records_container_move_fact_for_rewritten_set_index() {
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
        .position(|instr| instr.opcode() == Opcode32::SetIndex)
        .expect("SetIndex");
    let fact = function
        .performance
        .container_move(set_index_pc)
        .expect("container move fact");
    assert!(fact.move_key);
    assert!(fact.move_value);

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_keeps_local_set_index_key_readable() {
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
        .position(|instr| instr.opcode() == Opcode32::SetIndex)
        .expect("SetIndex");
    let fact = function
        .performance
        .container_move(set_index_pc)
        .expect("container move fact");
    assert!(!fact.move_key);
    assert!(fact.move_value);

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns.len(), 1);
    assert_eq!(result.returns[0].kind(), crate::val::RuntimeValKind::ShortStr);
}

#[test]
fn compiler32_records_set_index_target_shape_facts() {
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
        .filter(|(_, instr)| instr.opcode() == Opcode32::SetIndex)
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

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(46)]);
}

#[test]
fn compiler32_records_short_string_key_fact_for_set_index() {
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
        .filter(|(_, instr)| instr.opcode() == Opcode32::SetIndex)
        .filter_map(|(pc, _)| {
            function
                .performance
                .known_key(pc)
                .and_then(|fact| fact.const_key)
                .and_then(|key| function.consts.string(key))
        })
        .collect::<Vec<_>>();

    assert!(set_index_keys.contains(&"b"));
    assert!(set_index_keys.contains(&"score"));

    let result = execute32(&function).expect("execute");
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
    let list_result = execute32(&list_function).expect("execute list");
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
    let map_result = execute32(&map_function).expect("execute map");
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
fn compiler32_marks_pure_literal_expression_statement_as_dead_write() {
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
            matches!(instr.opcode(), Opcode32::LoadInt | Opcode32::LoadString) && function.performance.dead_write(*pc)
        })
        .count();
    assert_eq!(dead_loads, 2);

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_does_not_mark_heap_literal_expression_statement_as_dead_write() {
    let function = compile_source(
        r#"
        "longer-than-seven";
        return 42;
        "#,
    );

    let heap_const_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode32::LoadHeapConst)
        .expect("heap const load");
    assert!(!function.performance.dead_write(heap_const_pc));

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_records_short_string_key_fact_for_get_index() {
    let function = compile_source(r#"return {"answer": 42}.answer;"#);

    let get_index_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode32::GetIndex)
        .expect("GetIndex");
    let key_fact = function.performance.known_key(get_index_pc).expect("key fact");
    let const_key = key_fact.const_key.expect("const key");
    assert_eq!(function.consts.string(const_key), Some("answer"));

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_does_not_record_long_string_key_fact_for_get_index() {
    let function = compile_source(r#"return {"longer-than-seven": 42}."longer-than-seven";"#);

    let get_index_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode32::GetIndex)
        .expect("GetIndex");
    assert!(function.performance.known_key(get_index_pc).is_none());

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(42)]);
}

#[test]
fn compiler32_records_index_target_shape_facts() {
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
        .filter(|(_, instr)| instr.opcode() == Opcode32::GetIndex)
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

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(126)]);
}

#[test]
fn compiler32_records_control_flow_facts_after_jump_patching() {
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

    let test_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode32::Test)
        .expect("Test");
    let jmp_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode32::Jmp)
        .expect("Jmp");
    let test = function.code[test_pc];
    let jmp = function.code[jmp_pc];
    let test_taken = ((test_pc as i64) + 1 + i64::from(test.c() as i8)) as usize;
    let jmp_target = ((jmp_pc as i64) + 1 + i64::from(jmp.sj_arg())) as usize;

    assert!(function.performance.is_branch_target(test_pc + 1));
    assert!(function.performance.is_branch_target(test_taken));
    assert!(function.performance.is_branch_target(jmp_target));
    assert!(!function.performance.same_block(test_pc, test_pc + 1));
    assert!(!function.performance.same_block(test_pc + 1, test_taken));

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(1)]);
}

#[test]
fn compiler32_records_loop_backedge_as_branch_target() {
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
        .find(|(_, instr)| instr.opcode() == Opcode32::Jmp && instr.sj_arg() < 0)
        .expect("loop backedge");
    let target = ((loop_backedge.0 as i64) + 1 + i64::from(loop_backedge.1.sj_arg())) as usize;

    assert!(function.performance.is_branch_target(target));
    assert!(!function.performance.same_block(loop_backedge.0, target));

    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler32_records_positional_call_shape_fact() {
    let module = compile_source_module32(
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
        .position(|instr| instr.opcode() == Opcode32::Call)
        .expect("Call");
    let fact = function.performance.call_site(call_pc).expect("call fact");

    assert_eq!(fact.call_base, function.code[call_pc].a() as u16);
    assert_eq!(fact.positional_count, 2);
    assert_eq!(fact.named_count, 0);
    assert_eq!(fact.target_kind, PerfCallTargetKind::Closure);
}

#[test]
fn compiler32_records_dynamic_named_call_shape_fact() {
    let module = compile_source_module32(
        r#"
        let make = || |x| x;
        return make()(x: 42);
        "#,
    )
    .expect("compile module");
    let function = &module.functions[0];
    let call_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode32::CallNamed)
        .expect("CallNamed");
    let fact = function.performance.call_site(call_pc).expect("call fact");

    assert_eq!(fact.call_base, function.code[call_pc].a() as u16);
    assert_eq!(fact.positional_count, 0);
    assert_eq!(fact.named_count, 1);
    assert_eq!(fact.target_kind, PerfCallTargetKind::Unknown);
}

#[test]
fn compiler32_records_native_call_target_shape_fact() {
    fn native_id(args: NativeArgs32<'_>, _runtime: &mut NativeRuntime32<'_>) -> Result<RuntimeVal> {
        let [RuntimeVal::Int(value)] = args.as_slice() else {
            bail!("native_id expects one int");
        };
        Ok(RuntimeVal::Int(*value))
    }

    let module = compile_source_module_with_natives32(
        "return native_id(42);",
        vec![NativeEntry32 {
            name: "native_id".to_string(),
            arity: 1,
            function: NativeFunction32::Plain(native_id),
        }],
    )
    .expect("compile module");
    let function = &module.functions[0];
    let call_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode32::Call)
        .expect("Call");
    let fact = function.performance.call_site(call_pc).expect("call fact");

    assert_eq!(fact.target_kind, PerfCallTargetKind::Native);
}

#[test]
fn compiler32_records_global_slot_facts_for_get_and_set() {
    let module = compile_source_module32(
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
                .filter(|(_, instr)| matches!(instr.opcode(), Opcode32::GetGlobal | Opcode32::SetGlobal))
                .map(|(pc, instr)| (instr.opcode(), function.performance.global_op(pc).expect("global fact")))
        })
        .collect::<Vec<_>>();

    assert!(global_facts.iter().any(|(opcode, fact)| *opcode == Opcode32::GetGlobal
        && module.globals[fact.slot as usize].name.as_ref() == "counter"));
    assert!(global_facts.iter().any(|(opcode, fact)| *opcode == Opcode32::SetGlobal
        && module.globals[fact.slot as usize].name.as_ref() == "counter"));
}
