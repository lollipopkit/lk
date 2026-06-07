use super::*;

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

    let result = execute(&function).expect("execute");
    assert_eq!(returned_string(&result), "status=ok");
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
