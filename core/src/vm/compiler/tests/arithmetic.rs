use super::*;

#[test]
fn compiler_lowers_small_int_literal_add_sub_to_add_int_immediate() {
    let function = compile_source(
        r#"
        let total = 10;
        total += 1;
        let adjusted = total - 2;
        return adjusted;
        "#,
    )
    .expect("compile source");

    let immediate_count = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::AddIntI)
        .count();
    assert_eq!(
        immediate_count, 2,
        "small integer add/sub literals should lower to AddIntI: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(9)]);
}

#[test]
fn compiler_lowers_small_int_literal_mul_mod_to_int_immediates() {
    let function = compile_source(
        r#"
        let value = 10;
        let scaled = value * 3;
        let bucket = scaled % 7;
        return bucket;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::MulIntI),
        "small integer multiply literal should lower to MulIntI: {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::ModIntI),
        "small non-zero modulo literal should lower to ModIntI: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(2)]);
}

#[test]
fn compiler_lowers_commuted_small_int_add_mul_to_int_immediates() {
    let function = compile_source(
        r#"
        let value = 10;
        let offset = 2 + value;
        let scaled = 3 * offset;
        return scaled;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::AddIntI),
        "commuted small integer add literal should lower to AddIntI: {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::MulIntI),
        "commuted small integer multiply literal should lower to MulIntI: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(36)]);
}

#[test]
fn compiler_accumulates_int_add_chain_into_compound_target() {
    let function = compile_source(
        r#"
        let total = 10;
        let a = 2;
        let b = 3;
        let c = 4;
        let d = 5;
        let e = 6;
        let f = 7;
        total += (a * b) + (c * d) + (e * f);
        return total;
        "#,
    )
    .expect("compile source");

    let add_mul_count = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::AddMulInt)
        .count();
    assert_eq!(
        add_mul_count, 3,
        "compound add chain should fuse integer multiply terms into AddMulInt: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(78)]);
}

#[test]
fn compiler_keeps_compound_add_semantics_when_rhs_reads_target() {
    let function = compile_source(
        r#"
        let total = 10;
        total += total + total;
        return total;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(30)]);
}

#[test]
fn compiler_reuses_preloaded_loop_const_for_folded_compound_add_term() {
    let function = compile_source(
        r#"
        let total = 0;
        for i in 1..=3 {
            let a = 2;
            let b = 7;
            total += (a * b) + (i * 3);
        }
        return total;
        "#,
    )
    .expect("compile source");

    let add_mul_count = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::AddMulInt)
        .count();
    assert_eq!(
        add_mul_count, 2,
        "compound add terms should use AddMulInt inside loop body: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(60)]);
}

#[test]
fn compiler_accumulates_global_int_add_chain_before_set_global() {
    let module = compile_source_module(
        r#"
        checksum := 10;
        fn bump() {
            let a = 2;
            let b = 3;
            let c = 4;
            let d = 5;
            let e = 6;
            let f = 7;
            checksum += (a * b) + (c * d) + (e * f);
            return checksum;
        }
        return bump();
        "#,
    )
    .expect("compile module");
    let function = module
        .functions
        .iter()
        .find(|function| function.code.iter().any(|instr| instr.opcode() == Opcode::GetGlobal))
        .expect("function with global compound assignment");

    let add_mul_count = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::AddMulInt)
        .count();
    assert_eq!(
        add_mul_count, 3,
        "global compound add chain should fuse integer multiply terms into AddMulInt: {:?}",
        function.code
    );
    assert!(
        function
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::GetGlobal)),
        "global compound add chain should read the current global value: {:?}",
        function.code
    );
    assert!(
        function
            .code
            .iter()
            .any(|instr| matches!(instr.opcode(), Opcode::SetGlobal)),
        "global compound add chain should write the final global value: {:?}",
        function.code
    );

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(78)]);
}

#[test]
fn compiler_keeps_global_compound_add_semantics_when_rhs_reads_target() {
    let module = compile_source_module(
        r#"
        checksum := 10;
        checksum += checksum + checksum;
        return checksum;
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(30)]);
}
