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

/// A width fact does not outlive the value it described.
///
/// `machine_regs` is keyed by register, and registers are recycled: the one a
/// `u32` lived in inside a branch is handed to the next binding after it. If
/// the fact stayed, an unrelated `Int` would inherit it and wrap — a wrong
/// answer with nothing to point at, which is what the note in
/// `emit_bin_op_to_register_with_flavor` has always warned about.
///
/// Every site that writes a register used to be responsible for remembering to
/// clear. Now a binding clears its destination *before* anything is lowered
/// into it, and a move carries the source's width or clears it — so the fact is
/// established by whatever landed in the register, not by whatever was there
/// before.
#[test]
fn compiler_does_not_let_a_machine_width_outlive_its_value() {
    let module = compile_source_module(
        r#"
        fn narrow(flag: Bool) -> Int {
            if (flag) {
                let x: u32 = 4000000000;
                let y: u32 = 4000000000;
                return (x + y) as Int;
            }
            // The same registers, now holding plain integers. 8000000000 fits
            // in an Int and must not come back as a u32 sum.
            let p = 4000000000;
            let q = 4000000000;
            return p + q;
        }
        return narrow(false) + narrow(true);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    assert_eq!(
        result.returns,
        vec![crate::val::RuntimeVal::Int(8_000_000_000 + 3_705_032_704)],
        "the Int branch must not inherit the u32 branch's width"
    );
}

/// `1 + f(x)` evaluates `f(x)` **once**, in every syntactic position.
///
/// The immediate form of an int binary op wants the constant on the right, so
/// `const + expr` lowered `expr` first to ask whether its value is a proven
/// `Int`. When the answer was no — which it is for any call whose return type is
/// not annotated — the code fell through and lowered `expr` *again*, leaving the
/// first lowering's instructions in the stream. The operand then ran twice: `1 +
/// side(7)` called `side` twice, `f(5)` made 63 calls instead of 6, and `f(50)`
/// never finished at all. The *answer* stayed right for a pure function, which is
/// how it survived.
///
/// Both lowerings had it, and they cover different positions: `lower_into` takes
/// `let`/element/argument destinations, `lower_bin_op` takes `return` and template
/// interpolation. Fixing one and testing the other would have looked green, so the
/// count below spans both.
///
/// A *condition* (`if (1 + f(x) > 0)`) had a third mechanism for the same
/// wrongness and is counted here too: the condition path tries fused branch
/// shapes in turn, and each helper lowered an operand before checking a register
/// fact, so every rejected attempt left its instructions in the stream — three
/// evaluations, one per attempt that looked and declined. The attempts are now
/// restricted to operands that are free to lower twice
/// (`is_free_to_lower_twice`), which is what makes speculation-by-lowering sound
/// at all.
///
/// Pinned to a *number*, not to the other backend, because **no differential
/// test can see this**: both backends lower from this bytecode, so both doubled
/// the call identically and agreed with each other. A VM-vs-native comparison is
/// blind to a front-end bug by construction.
#[test]
fn a_commuted_immediate_evaluates_its_operand_once() {
    let module = crate::vm::compile_source_module(
        r#"
        let calls = 0;
        fn side(x) { calls = calls + 1; return x; }
        fn through_return() { return 1 + side(1); }
        fn through_condition() { if (1 + side(1) > 0) { return 0; } return 0; }
        fn through_while() { while (1 + side(1) > 99) { return 0; } return 0; }
        let through_let = 1 + side(1);
        let through_element = [2 * side(1)];
        let through_template = "${1 + side(1)}";
        through_return();
        through_condition();
        through_while();
        return calls;
        "#,
    )
    .expect("compile module");

    let result = crate::vm::execute_module(&module).expect("run module");
    assert_eq!(
        result.returns,
        vec![crate::val::RuntimeVal::Int(6)],
        "six `side` calls are written, so six must run — the doubling made this 7, 9, 12 or more \
         depending on which positions were involved"
    );

    // The same claim on the instruction stream, where it is a property of the
    // code rather than of one run: the entry emits exactly the calls the source
    // spells.
    let entry = module.entry_function().expect("entry function");
    let emitted = entry
        .code
        .iter()
        // Any call-shaped opcode, by name: the lowering picks between
        // `CallDirect`/`Call`/`CallNamed` on grounds this test does not care
        // about, and matching an explicit list would let a new one through
        // silently.
        .filter(|instr| alloc::format!("{:?}", instr.opcode()).starts_with("Call"))
        .count();
    assert_eq!(
        emitted,
        6,
        "three `side` operands in the entry plus the three calls to the helpers: {:?}",
        entry.code.iter().map(|i| i.opcode()).collect::<Vec<_>>()
    );
}
