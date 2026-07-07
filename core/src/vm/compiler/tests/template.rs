use super::*;
#[cfg(not(feature = "std"))]
use crate::compat::prelude::*;

fn load_int_register(function: &Function, value: i64) -> u8 {
    function
        .code
        .iter()
        .find_map(|instr| {
            (instr.opcode() == Opcode::LoadInt && function.consts.int(instr.bx()) == Some(value)).then_some(instr.a())
        })
        .expect("LoadInt register")
}

fn load_string_count(function: &Function, value: &str) -> usize {
    function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::LoadString && function.consts.string(instr.bx()) == Some(value))
        .count()
}

fn opcode_count(function: &Function, opcode: Opcode) -> usize {
    function.code.iter().filter(|instr| instr.opcode() == opcode).count()
}

fn returned_string(result: &crate::vm::ExecResult) -> String {
    match &result.returns[0] {
        crate::val::RuntimeVal::ShortStr(value) => value.as_str().to_string(),
        crate::val::RuntimeVal::Obj(handle) => match result.state.heap.get(*handle).expect("heap value") {
            crate::val::HeapValue::String(value) => value.to_string(),
            other => panic!("expected heap string, got {other:?}"),
        },
        other => panic!("expected string, got {:?}", other.kind()),
    }
}

#[test]
fn compiler_template_string_starts_from_first_part() {
    let function = compile_source(
        r#"
        let bucket = 7;
        let key = "b${bucket}";
        return key;
        "#,
    )
    .expect("compile source");

    assert_eq!(load_string_count(&function, ""), 0);
    assert_eq!(opcode_count(&function, Opcode::ToString), 0);
    assert_eq!(opcode_count(&function, Opcode::ConcatString), 1);
    assert_eq!(opcode_count(&function, Opcode::ConcatN), 0);

    let bucket = load_int_register(&function, 7);
    let concat = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::ConcatString)
        .expect("ConcatString");
    assert_eq!(
        concat.c(),
        bucket,
        "template concat should read the local interpolation directly instead of materializing ToString"
    );

    let result = execute(&function).expect("execute");
    assert_eq!(returned_string(&result), "b7");
}

#[test]
fn compiler_template_string_skips_to_string_for_string_parts() {
    let function = compile_source(
        r#"
        let status = "ok";
        let line = "status=${status}";
        return line;
        "#,
    )
    .expect("compile source");

    assert_eq!(load_string_count(&function, ""), 0);
    assert_eq!(opcode_count(&function, Opcode::ToString), 0);
    assert_eq!(opcode_count(&function, Opcode::ConcatString), 1);
    assert_eq!(opcode_count(&function, Opcode::ConcatN), 0);

    let result = execute(&function).expect("execute");
    assert_eq!(returned_string(&result), "status=ok");
}

#[test]
fn compiler_template_string_assignment_writes_directly_to_destination() {
    let function = compile_source(
        r#"
        let bucket = 7;
        let key = "b${bucket}";
        return key;
        "#,
    )
    .expect("compile source");

    let concat = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::ConcatString)
        .expect("ConcatString");
    let ret = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::Return1)
        .expect("Return1");

    assert_eq!(
        concat.a(),
        ret.a(),
        "template assignment should write directly to the returned local: {:?}",
        function.code
    );
    assert!(
        !function
            .code
            .windows(2)
            .any(|window| window[0].opcode() == Opcode::ConcatString
                && window[1].opcode() == Opcode::Move
                && window[0].a() == window[1].b()),
        "template key assignment should not move the ConcatString result into its destination: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(returned_string(&result), "b7");
}

#[test]
fn compiler_lowers_template_string_int_map_set_without_key_materialization() {
    let function = compile_source(
        r#"
        let values = {};
        let bucket = 7;
        values.set("n${bucket}", 40);
        return values["n7"];
        "#,
    )
    .expect("compile source");

    assert_eq!(opcode_count(&function, Opcode::SetIndexStrI), 1);
    assert_eq!(opcode_count(&function, Opcode::ConcatString), 0);
    assert_eq!(opcode_count(&function, Opcode::ConcatN), 0);

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(40)]);
}

#[test]
fn compiler_keeps_dynamic_template_map_key_when_suffix_is_not_int() {
    let function = compile_source(
        r#"
        let values = {};
        let suffix = "x";
        values.set("n${suffix}", 40);
        return values["nx"];
        "#,
    )
    .expect("compile source");

    assert_eq!(opcode_count(&function, Opcode::SetIndexStrI), 0);
    assert_eq!(opcode_count(&function, Opcode::ConcatString), 1);

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(40)]);
}

#[test]
fn compiler_template_string_uses_concat_n_for_three_or_more_parts() {
    let function = compile_source(
        r#"
        let name = "api";
        let shard = 7;
        let key = "svc:${name}:${shard}";
        return key;
        "#,
    )
    .expect("compile source");

    assert_eq!(load_string_count(&function, ""), 0);
    assert_eq!(opcode_count(&function, Opcode::ToString), 0);
    assert_eq!(opcode_count(&function, Opcode::ConcatString), 0);
    assert_eq!(opcode_count(&function, Opcode::ConcatN), 1);

    let result = execute(&function).expect("execute");
    assert_eq!(returned_string(&result), "svc:api:7");
}

#[test]
fn compiler_lowers_template_expression_parts_directly_into_concat_window() {
    let function = compile_source(
        r#"
        let x = 42;
        let key = "id:${x % 97}:${x * 2}";
        return key;
        "#,
    )
    .expect("compile source");

    let concat = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::ConcatN)
        .expect("ConcatN");
    let start = concat.b();
    let end = start + concat.c();
    let window_moves = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::Move && instr.a() >= start && instr.a() < end)
        .count();

    assert_eq!(
        window_moves, 0,
        "template arithmetic parts should lower directly into the ConcatN window: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(returned_string(&result), "id:42:84");
}

#[test]
fn compiler_template_string_preserves_to_string_for_single_expression() {
    let function = compile_source(
        r#"
        let bucket = 7;
        return "${bucket}";
        "#,
    )
    .expect("compile source");

    assert_eq!(opcode_count(&function, Opcode::ToString), 1);
    assert_eq!(opcode_count(&function, Opcode::ConcatString), 0);

    let result = execute(&function).expect("execute");
    assert_eq!(returned_string(&result), "7");
}
