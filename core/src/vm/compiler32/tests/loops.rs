use super::*;

#[test]
fn compiler32_for_over_local_string_does_not_clone_iterable_local() {
    let function = compile_source32(
        r#"
        let s = "tenant-123-order-45";
        let total = 0;
        for ch in s {
            total += ch.len();
        }
        return total;
        "#,
    )
    .expect("compile source");

    crate::vm::vm_runtime_metrics_reset();
    let result = execute32(&function).expect("execute");
    let metrics = crate::vm::vm_runtime_metrics_snapshot();

    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode32::ToIter),
        "string for loop should index the string directly instead of materializing a char list"
    );
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(19)]);
    assert_eq!(
        metrics.local_store_heap_clones, 0,
        "readonly for iterable should use the local string slot directly"
    );
}

#[test]
fn compiler32_for_over_template_string_indexes_directly() {
    let function = compile_source32(
        r#"
        let i = 42;
        let s = "tenant-${i}-region";
        let total = 0;
        for ch in s {
            total += ch.len();
        }
        return total;
        "#,
    )
    .expect("compile source");

    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode32::ToIter),
        "template string for loop should use string Len/GetIndex directly"
    );
    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(16)]);
}

#[test]
fn compiler32_lowers_for_over_list_with_indexed_len_path() {
    let function = compile_source32(
        r#"
        let sum = 0;
        for value in [1, 2, 3, 4] {
            sum = sum + value;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode32::Len),
        "expected Len in {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode32::ToIter),
        "list for loop should index the list directly instead of normalizing through ToIter"
    );

    let result = execute32(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler32_lowers_for_tuple_pattern_over_map_entries() {
    let function = compile_source32(
        r#"
        let total = 0;
        let items = {"a": 1, "b": 2};
        for (key, value) in items {
            total = total + value;
        }
        return total;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode32::ToIter),
        "expected ToIter in {:?}",
        function.code
    );

    let result = execute32(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler32_lowers_for_over_short_string_with_indexed_len_path() {
    let function = compile_source32(
        r#"
        let count = 0;
        for ch in "abc" {
            count = count + 1;
        }
        return count;
        "#,
    )
    .expect("compile source");

    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode32::ToIter),
        "string literal for loop should index the string directly"
    );
    let result = execute32(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler32_lowers_for_range_with_break_and_continue() {
    let function = compile_source32(
        r#"
        let sum = 0;
        for i in 0..7 {
            if (i == 3) {
                continue;
            }
            if (i == 6) {
                break;
            }
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    let result = execute32(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(12)]);
}

#[test]
fn compiler32_lowers_default_positive_for_range_without_dynamic_step_sign() {
    let function = compile_source32(
        r#"
        let sum = 0;
        for i in 0..5 {
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode32::CmpGtInt),
        "default positive range step should not emit per-iteration step sign checks"
    );
    let first_cmp = function
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode32::CmpLtInt | Opcode32::CmpLeInt))
        .expect("range condition");
    assert!(
        !function.code[..first_cmp]
            .iter()
            .any(|instr| instr.opcode() == Opcode32::Move),
        "range literal start should lower directly into the loop index slot"
    );
    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler32_lowers_for_range_inclusive_and_negative_step() {
    let function = compile_source32(
        r#"
        let sum = 0;
        for i in 5..=1..0 - 2 {
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    let result = execute32(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(9)]);
}

#[test]
fn compiler32_keeps_dynamic_for_range_step_sign_fallback() {
    let function = compile_source32(
        r#"
        let sum = 0;
        let step = 1;
        for i in 0..5..step {
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode32::CmpGtInt),
        "dynamic range step still needs runtime sign dispatch"
    );
    let result = execute32(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}
