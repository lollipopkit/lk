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
fn compiler_accumulates_plain_int_add_pairs_into_compound_target() {
    let function = compile_source(
        r#"
        let total = 10;
        let a = 1;
        let b = 2;
        let c = 3;
        total += a + b + c;
        return total;
        "#,
    )
    .expect("compile source");

    let add2_count = function
        .code
        .iter()
        .filter(|instr| instr.opcode() == Opcode::Add2Int)
        .count();
    assert_eq!(
        add2_count, 1,
        "compound add chain should fuse adjacent integer terms into Add2Int: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(16)]);
}

#[test]
fn compiler_accumulates_typed_int_list_access_in_place() {
    let function = compile_source(
        r#"
        let values = [];
        for i in 0..3 {
            values.push(i + 1);
        }
        let total = 0;
        for i in 0..3 {
            total += values[i];
            if i > 0 {
                total -= values[i - 1];
            }
        }
        return total;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::AddListInt),
        "typed int list add accumulator should lower to AddListInt: {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::SubListInt),
        "typed int list sub accumulator should lower to SubListInt: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
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

/// A machine int wraps whether or not its width was written down.
///
/// The wrap is emitted where the width is *proven*, and proof used to come from
/// exactly two places: an annotation and an `as` cast. A value whose width came
/// from anywhere else — a function that declares a machine return, a builtin
/// whose name is a width, a read of a local already known to hold one — was
/// left unproven and did not wrap.
///
/// So these two computed different numbers from the same types and the same
/// values, and which one you got depended on whether a width had been typed
/// out. 4000000000 + 4000000000 is 8000000000, and as a `u32` it is 3705032704.
#[test]
fn compiler_wraps_machine_ints_whose_width_was_inferred() {
    let module = compile_source_module(
        r#"
        fn read() -> u32 { return 4000000000 as u32; }
        let inferred_a = read();
        let inferred_b = read();
        let annotated_a: u32 = 4000000000;
        let annotated_b: u32 = 4000000000;
        let through_a = annotated_a;
        let through_b = annotated_b;
        // Summed rather than listed, so the assertion is one number: any of
        // the three failing to wrap makes it too big by a known amount.
        return (inferred_a + inferred_b) as Int
             + (annotated_a + annotated_b) as Int
             + (through_a + through_b) as Int;
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    assert_eq!(
        result.returns,
        vec![crate::val::RuntimeVal::Int(3 * 3_705_032_704)],
        "a u32 sum must wrap the same way however its width was learned"
    );
}
