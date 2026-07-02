use super::*;

#[test]
fn compiler_for_over_local_string_does_not_clone_iterable_local() {
    let function = compile_source(
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
    let result = execute(&function).expect("execute");
    let metrics = crate::vm::vm_runtime_metrics_snapshot();

    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::ToIter),
        "string for loop should index the string directly instead of materializing a char list"
    );
    assert_eq!(
        function
            .code
            .iter()
            .filter(|instr| instr.opcode() == Opcode::Len)
            .count(),
        1,
        "string iteration should keep only iterable length; ch.len() is statically 1: {:?}",
        function.code
    );
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(19)]);
    assert_eq!(
        metrics.local_store_heap_clones, 0,
        "readonly for iterable should use the local string slot directly"
    );
}

#[test]
fn compiler_elides_string_for_value_when_only_len_is_used() {
    let function = compile_source(
        r#"
        let s = "tenant-123";
        let total = 0;
        for ch in s {
            total += ch.len();
        }
        return total;
        "#,
    )
    .expect("compile source");

    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::GetIndex),
        "single-character value should not be materialized when only len() is used: {:?}",
        function.code
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler_keeps_string_for_value_when_binding_is_read() {
    let function = compile_source(
        r#"
        let s = "abc";
        let total = 0;
        for ch in s {
            let copy = ch;
            total += copy.len();
        }
        return total;
        "#,
    )
    .expect("compile source");

    assert!(
        function.code.iter().any(|instr| instr.opcode() == Opcode::GetIndex),
        "reading the loop binding still requires materializing the character: {:?}",
        function.code
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler_for_over_template_string_indexes_directly() {
    let function = compile_source(
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
        !function.code.iter().any(|instr| instr.opcode() == Opcode::ToIter),
        "template string for loop should use string Len/GetIndex directly"
    );
    assert_eq!(
        function
            .code
            .iter()
            .filter(|instr| instr.opcode() == Opcode::Len)
            .count(),
        1,
        "template string iteration should not emit Len for ch.len(): {:?}",
        function.code
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(16)]);
}

#[test]
fn compiler_string_for_len_fact_does_not_survive_shadowing() {
    let function = compile_source(
        r#"
        let s = "abc";
        let total = 0;
        for ch in s {
            let ch = "wide";
            total += ch.len();
        }
        return total;
        "#,
    )
    .expect("compile source");

    assert!(
        function
            .code
            .iter()
            .filter(|instr| instr.opcode() == Opcode::Len)
            .count()
            >= 2,
        "shadowed ch.len() must keep dynamic/string length semantics: {:?}",
        function.code
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(12)]);
}

#[test]
fn compiler_lowers_for_over_list_with_indexed_len_path() {
    let function = compile_source(
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
        function.code.iter().any(|instr| instr.opcode() == Opcode::Len),
        "expected Len in {:?}",
        function.code
    );
    assert!(
        !function.code.iter().any(|instr| instr.opcode() == Opcode::ToIter),
        "list for loop should index the list directly instead of normalizing through ToIter"
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler_lowers_for_tuple_pattern_over_map_entries() {
    let function = compile_source(
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
        function.code.iter().any(|instr| instr.opcode() == Opcode::ToIter),
        "expected ToIter in {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler_lowers_for_over_short_string_with_indexed_len_path() {
    let function = compile_source(
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
        !function.code.iter().any(|instr| instr.opcode() == Opcode::ToIter),
        "string literal for loop should index the string directly"
    );
    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler_lowers_for_range_with_break_and_continue() {
    let function = compile_source(
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

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(12)]);
}

#[test]
fn compiler_lowers_default_positive_for_range_without_dynamic_step_sign() {
    let function = compile_source(
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
        !function.code.iter().any(|instr| instr.opcode() == Opcode::CmpGtInt),
        "default positive range step should not emit per-iteration step sign checks"
    );
    let first_cmp = function
        .code
        .iter()
        .position(|instr| matches!(instr.opcode(), Opcode::CmpLtInt | Opcode::CmpLeInt))
        .expect("range condition");
    assert!(
        !function.code[..first_cmp]
            .iter()
            .any(|instr| instr.opcode() == Opcode::Move),
        "range literal start should lower directly into the loop index slot"
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler_lowers_for_range_inclusive_and_negative_step() {
    let function = compile_source(
        r#"
        let sum = 0;
        for i in 5..=1..0 - 2 {
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    let result = execute(&function).expect("execute");

    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(9)]);
}

#[test]
fn compiler_keeps_dynamic_for_range_step_sign_fallback() {
    let function = compile_source(
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
        function.code.iter().any(|instr| instr.opcode() == Opcode::CmpGtInt),
        "dynamic range step still needs runtime sign dispatch"
    );
    let step_reg = first_const_int_register(&function, 1);
    let step_sign_check = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::CmpGtInt)
        .expect("dynamic step sign check");
    assert_eq!(
        step_sign_check.b(),
        step_reg,
        "unmutated local range step should be read directly from its local slot"
    );
    let step_sign_pc = function
        .code
        .iter()
        .position(|instr| std::ptr::eq(instr, step_sign_check))
        .expect("step sign pc");
    let zero_load_pc = load_int_register_pc(&function, step_sign_check.c(), 0).expect("step sign zero load");
    let loop_target = first_backward_loop_target_after(&function, step_sign_pc);
    assert!(
        zero_load_pc as i64 <= loop_target - 1,
        "dynamic range step zero should be loaded before the loop-back target"
    );
    assert!(
        !moves_from_range_condition_register(&function, step_sign_pc),
        "dynamic range should branch on direction-specific conditions instead of moving them into a merged condition"
    );
    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler_for_range_reuses_unmutated_local_end() {
    let function = compile_source(
        r#"
        let limit = 5;
        let sum = 0;
        for i in 0..limit {
            sum += i;
        }
        return sum;
        "#,
    )
    .expect("compile source");

    let limit_reg = first_const_int_register(&function, 5);
    let condition = first_range_condition(&function);
    assert_eq!(
        condition.c(),
        limit_reg,
        "unmutated local range end should be read directly from its local slot"
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler_for_range_reuses_body_scalar_literals_before_loop_target() {
    let function = compile_source(
        r#"
        let total = 0;
        for i in 0..10 {
            total += 1;
            if (i == 5) {
                total += 10;
            }
        }
        return total;
        "#,
    )
    .expect("compile source");

    let condition_pc = function
        .code
        .iter()
        .position(|instr| {
            matches!(
                instr.opcode(),
                Opcode::CmpLtInt | Opcode::TestLtInt | Opcode::TestLtIntI
            )
        })
        .expect("expected range condition");
    let loop_target = first_backward_loop_target_after(&function, condition_pc);

    assert!(
        load_int_pcs(&function, 1)
            .into_iter()
            .all(|pc| pc as i64 <= loop_target - 1),
        "for range scalar const cache should preload literal 1 before loop target {loop_target}; code: {:?}",
        function.code,
    );
    assert!(
        load_int_pcs(&function, 10)
            .into_iter()
            .all(|pc| pc as i64 <= loop_target - 1),
        "for range scalar const cache should preload literal 10 before loop target {loop_target}; code: {:?}",
        function.code,
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(20)]);
}

#[test]
fn compiler_for_range_cached_literals_are_not_consumed_by_set_index() {
    let function = compile_source(
        r#"
        let hist = {};
        for i in 1..=4 {
            let key = "b${i % 2}";
            let prev = hist[key];
            if prev == nil {
                hist[key] = 1;
            } else {
                hist[key] = prev + 1;
            }
        }
        return hist["b0"] + hist["b1"];
        "#,
    )
    .expect("compile source");

    let set_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::SetIndex)
        .expect("expected SetIndex");
    let move_fact = function.performance.container_move(set_pc).expect("SetIndex move fact");
    assert!(
        !move_fact.move_value,
        "SetIndex must not consume loop cached literal value registers"
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(4)]);
}

#[test]
fn compiler_for_range_snapshots_mutated_local_end() {
    let function = compile_source(
        r#"
        let limit = 5;
        let count = 0;
        for i in 0..limit {
            count += 1;
            limit = 2;
        }
        return count;
        "#,
    )
    .expect("compile source");

    let limit_reg = first_const_int_register(&function, 5);
    let condition = first_range_condition(&function);
    assert_ne!(
        condition.c(),
        limit_reg,
        "mutated local range end must be snapshotted before the loop body can change it"
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(5)]);
}

#[test]
fn compiler_for_range_snapshots_mutated_local_step() {
    let function = compile_source(
        r#"
        let step = 1;
        let count = 0;
        for i in 0..5..step {
            count += 1;
            step = 2;
        }
        return count;
        "#,
    )
    .expect("compile source");

    let step_reg = first_const_int_register(&function, 1);
    let step_sign_check = function
        .code
        .iter()
        .find(|instr| instr.opcode() == Opcode::CmpGtInt)
        .expect("dynamic step sign check");
    assert_ne!(
        step_sign_check.b(),
        step_reg,
        "mutated local range step must be snapshotted before the loop body can change it"
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(5)]);
}

#[test]
fn compiler_while_licm_hoists_constant_loads_out_of_loop() {
    // The condition `b != 0` contains a constant LoadInt for the literal 0.
    // With LICM, the LoadInt should appear before the loop-back target,
    // so the loop entry (where Jmp back lands) skips the constant load.
    let function = compile_source(
        r#"
        let a = 48;
        let b = 18;
        while (b != 0) {
            let t = a % b;
            a = b;
            b = t;
        }
        return a;
        "#,
    )
    .expect("compile source");

    // Find the LoadInt for the constant 0 in the condition
    let cmp_idx = function
        .code
        .iter()
        .position(|instr| {
            matches!(
                instr.opcode(),
                Opcode::CmpNeInt | Opcode::TestNeInt | Opcode::TestNeIntI | Opcode::BrEqZeroInt | Opcode::BrNeZeroInt
            )
        })
        .expect("expected CmpNeInt/TestNeInt/TestNeIntI/zero branch");

    // Find the Jmp that goes backward (loop back)
    let jmp_back_idx = function
        .code
        .iter()
        .enumerate()
        .filter(|(_, instr)| instr.opcode() == Opcode::Jmp)
        .find_map(|(i, instr)| {
            let offset = instr.sj_arg();
            if offset < 0 && i > cmp_idx { Some(i) } else { None }
        })
        .expect("expected a backward Jmp (loop back)");

    let jmp_offset = function.code[jmp_back_idx].sj_arg();
    let jmp_target = jmp_back_idx as i64 + 1 + jmp_offset as i64;

    // With LICM or immediate compare-test, the loop-back target should skip
    // condition constant loads and target cmp_idx or later.
    assert!(
        jmp_target >= cmp_idx as i64,
        "LICM: loop-back Jmp at {} targets {} but should skip condition literal loads (target should be >= {})",
        jmp_back_idx,
        jmp_target,
        cmp_idx,
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(6)]);
}

#[test]
fn compiler_while_reuses_body_scalar_literals_before_loop_target() {
    let function = compile_source(
        r#"
        let i = 0;
        let score = 0;
        while (i < 10) {
            score += 1;
            if (i == 5) {
                score += 10;
            }
            i += 1;
        }
        return score;
        "#,
    )
    .expect("compile source");

    let cmp_pc = function
        .code
        .iter()
        .position(|instr| {
            matches!(
                instr.opcode(),
                Opcode::CmpLtInt | Opcode::TestLtInt | Opcode::TestLtIntI
            )
        })
        .expect("expected while condition");
    let loop_target = first_backward_loop_target_after(&function, cmp_pc);

    assert!(
        load_int_pcs(&function, 1)
            .into_iter()
            .all(|pc| pc as i64 <= loop_target - 1),
        "loop scalar const cache should preload literal 1 before loop target {loop_target}; code: {:?}",
        function.code,
    );
    assert!(
        load_int_pcs(&function, 10)
            .into_iter()
            .all(|pc| pc as i64 <= loop_target - 1),
        "loop scalar const cache should preload literal 10 before loop target {loop_target}; code: {:?}",
        function.code,
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(20)]);
}

#[test]
fn compiler_binds_loop_local_literal_to_cached_register_with_copy_on_write() {
    let function = compile_source(
        r#"
        let i = 0;
        let total = 0;
        while (i < 3) {
            let one = 1;
            total += one;
            one = 2;
            let also_one = 1;
            total += one + also_one;
            i += 1;
        }
        return total;
        "#,
    )
    .expect("compile source");

    let cmp_pc = function
        .code
        .iter()
        .position(|instr| {
            matches!(
                instr.opcode(),
                Opcode::CmpLtInt | Opcode::TestLtInt | Opcode::TestLtIntI
            )
        })
        .expect("expected while condition");
    let loop_target = first_backward_loop_target_after(&function, cmp_pc) as usize;
    let cached_one = function
        .code
        .iter()
        .take(loop_target)
        .find(|instr| instr.opcode() == Opcode::LoadInt && function.consts.int(instr.bx()) == Some(1))
        .map(|instr| instr.a())
        .expect("expected loop cached literal 1");

    assert!(
        function
            .code
            .iter()
            .skip(loop_target)
            .all(|instr| instr.opcode() != Opcode::Move || instr.b() != cached_one),
        "loop-local let literal should bind to cached register without per-iteration Move: {:?}",
        function.code,
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(12)]);
}

#[test]
fn compiler_while_licm_keeps_heap_const_loads_on_loop_path() {
    let function = compile_source(
        r#"
        let i = 0;
        while ([1, 2].len() > 0) {
            i += 1;
            if (i > 2) {
                break;
            }
        }
        return i;
        "#,
    )
    .expect("compile source");

    let heap_const_idx = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::LoadHeapConst)
        .expect("expected LoadHeapConst for list literal");
    let len_idx = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::Len)
        .expect("expected Len for list literal condition");
    let loop_target = first_backward_loop_target_after(&function, len_idx);

    assert_eq!(
        loop_target, heap_const_idx as i64,
        "loop-back target must include mutable heap constant load at {heap_const_idx}"
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(3)]);
}

#[test]
fn compiler_range_loop_caches_direct_inline_body_literals() {
    let module = compile_source_module(
        r#"
        fn classify(n) {
            if n == 0 {
                return 1;
            }
            return 2;
        }

        let total = 0;
        for i in 1..=5 {
            total += classify(i);
        }
        return total;
        "#,
    )
    .expect("compile source");
    let function = module.entry_function().expect("entry function");
    let for_loop_pc = function
        .code
        .iter()
        .position(|instr| instr.opcode() == Opcode::ForLoopI)
        .expect("expected ForLoopI");
    let body_start = first_backward_loop_target_after(function, 0) as usize;

    for value in [0, 1, 2] {
        let in_body = function.code[body_start..for_loop_pc]
            .iter()
            .any(|instr| instr.opcode() == Opcode::LoadInt && function.consts.int(instr.bx()) == Some(value));
        assert!(
            !in_body,
            "direct-inline literal {value} should be cached before loop body: {:?}",
            function.code
        );
    }

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(10)]);
}

#[test]
fn compiler_caches_template_literal_parts_before_loop_body() {
    let function = compile_source(
        r#"
        let i = 0;
        let total = 0;
        while (i < 4) {
            let key = "order-${i}";
            total += key.len();
            i += 1;
        }
        return total;
        "#,
    )
    .expect("compile source");

    let cmp_pc = function
        .code
        .iter()
        .position(|instr| {
            matches!(
                instr.opcode(),
                Opcode::CmpLtInt | Opcode::TestLtInt | Opcode::TestLtIntI
            )
        })
        .expect("expected while condition");
    let loop_target = first_backward_loop_target_after(&function, cmp_pc) as usize;
    let prefix_loads = function
        .code
        .iter()
        .enumerate()
        .filter(|(_, instr)| {
            instr.opcode() == Opcode::LoadString && function.consts.string(instr.bx()) == Some("order-")
        })
        .map(|(pc, _)| pc)
        .collect::<Vec<_>>();

    assert!(
        !prefix_loads.is_empty(),
        "template literal prefix should be cached as a loop scalar const: {:?}",
        function.code
    );
    assert!(
        prefix_loads.iter().all(|pc| *pc < loop_target),
        "template literal prefix should not be loaded inside the loop body: {:?}",
        function.code
    );

    let result = execute(&function).expect("execute");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(28)]);
}

fn first_backward_loop_target_after(function: &Function, pc: usize) -> i64 {
    function
        .code
        .iter()
        .enumerate()
        .skip(pc + 1)
        .find_map(|(i, instr)| match instr.opcode() {
            Opcode::Jmp => {
                let offset = instr.sj_arg();
                (offset < 0).then_some(i as i64 + 1 + offset as i64)
            }
            Opcode::ForLoopI => {
                let offset = function.performance.for_loop(i)?.jump_offset;
                (offset < 0).then_some(i as i64 + 1 + offset as i64)
            }
            _ => None,
        })
        .expect("expected a backward loop branch")
}

fn first_const_int_register(function: &Function, value: i64) -> u8 {
    function
        .code
        .iter()
        .find_map(|instr| {
            if instr.opcode() == Opcode::LoadInt && function.consts.int(instr.bx()) == Some(value) {
                Some(instr.a())
            } else {
                None
            }
        })
        .expect("expected const int load")
}

fn load_int_register_pc(function: &Function, register: u8, value: i64) -> Option<usize> {
    function.code.iter().enumerate().find_map(|(pc, instr)| {
        if instr.opcode() == Opcode::LoadInt && instr.a() == register && function.consts.int(instr.bx()) == Some(value)
        {
            Some(pc)
        } else {
            None
        }
    })
}

fn load_int_pcs(function: &Function, value: i64) -> Vec<usize> {
    function
        .code
        .iter()
        .enumerate()
        .filter_map(|(pc, instr)| {
            if instr.opcode() == Opcode::LoadInt && function.consts.int(instr.bx()) == Some(value) {
                Some(pc)
            } else {
                None
            }
        })
        .collect()
}

fn first_range_condition(function: &Function) -> Instr {
    function
        .code
        .iter()
        .copied()
        .find(|instr| matches!(instr.opcode(), Opcode::CmpLtInt | Opcode::CmpLeInt))
        .expect("expected positive range condition")
}

fn moves_from_range_condition_register(function: &Function, sign_pc: usize) -> bool {
    let mut condition_regs = Vec::new();
    for instr in function.code.iter().skip(sign_pc + 1) {
        match instr.opcode() {
            Opcode::CmpLtInt | Opcode::CmpLeInt | Opcode::CmpGeInt | Opcode::CmpGtInt => {
                condition_regs.push(instr.a());
            }
            Opcode::AddInt => break,
            _ => {}
        }
    }
    function
        .code
        .iter()
        .any(|instr| instr.opcode() == Opcode::Move && condition_regs.contains(&instr.b()))
}

#[test]
fn compiler_while_loop_captured_outer_local_promotes_at_loop_entry() {
    // The capture promotion must run before the condition is emitted: a
    // promotion emitted mid-body re-executes each iteration (orphaning the
    // shared cell) and the condition's raw register read breaks on the back
    // edge once the register holds the cell. Regression: iteration 2 failed
    // with "expected Int, got Obj".
    let module = compile_source_module(
        r#"
        let i = 0;
        let total = 0;
        while (i < 3) {
            let f = |x| x + i;
            total = total + f(10);
            i = i + 1;
        }
        return total;
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    // Shared-cell doctrine: each call sees the current i (10 + 11 + 12).
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(33)]);
}

#[test]
fn compiler_range_for_loop_variable_captures_as_snapshot_cell() {
    // A `for` loop variable cannot be re-bound to a cell (the fused loop
    // opcode drives the raw register): each capture copies the counter into
    // a fresh snapshot cell — and must *copy*, not move (regression: the
    // counter register became Nil and ForLoopI failed).
    let module = compile_source_module(
        r#"
        let total = 0;
        for i in 0..3 {
            let f = |x| x + i;
            total = total + f(10);
        }
        return total;
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(33)]);
}

#[test]
fn compiler_loop_escaping_closure_sees_final_cell_value() {
    // A closure assigned inside the loop and called after it reads the
    // shared cell's final content (the capture is one cell across
    // iterations, pre-promoted at loop entry).
    let module = compile_source_module(
        r#"
        let i = 0;
        let g = |x| x;
        while (i < 3) {
            g = |x| x + i;
            i = i + 1;
        }
        return g(100);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(103)]);
}

#[test]
fn compiler_redeclared_promoted_name_reads_as_plain_value() {
    // A fresh `let` of a name whose previous binding was promoted to a cell
    // must drop the stale cell mark: reads of the new binding are plain
    // values. Regression: "LoadCellVal expected UpvalCell object".
    let module = compile_source_module(
        r#"
        let a = 1;
        let f = |x| x + a;
        let a = 2;
        return a * 10 + f(0);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    // New binding reads 2; the closure keeps the first binding's cell (1).
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(21)]);
}

#[test]
fn compiler_loop_variable_shadowing_promoted_outer_name() {
    // The loop binding is fresh (clears the outer promotion mark for the
    // body) and the restore re-instates both the outer slot and its cell
    // mark, so post-loop reads still go through the cell. Regression:
    // "LoadCellVal expected UpvalCell object" on the loop body read.
    let module = compile_source_module(
        r#"
        let i = 100;
        let g = |x| x + i;
        let total = 0;
        for i in 0..2 {
            total = total + i;
        }
        i = 200;
        return total * 1000 + g(0);
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    // total = 0 + 1; the escaped closure sees the post-loop mutation (200).
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(1200)]);
}

#[test]
fn compiler_loop_body_re_let_of_loop_name_is_a_fresh_binding() {
    // `let i = …` inside `for i` is a fresh ordinary local: it must not
    // clobber the counter register the fused loop opcode drives (the loop
    // ran once instead of twice), and a closure capturing it promotes a
    // shared cell — the later assignment stays visible (regression: the
    // capture was misclassified as a loop-variable snapshot by name).
    let module = compile_source_module(
        r#"
        let total = 0;
        for i in 0..2 {
            let i = i * 10;
            let f = |x| x + i;
            i = i + 5;
            total = total * 100 + f(0);
        }
        return total;
        "#,
    )
    .expect("compile module");

    let result = execute_module(&module).expect("execute module");
    // Iterations: 5 then 15 → 5 * 100 + 15.
    assert_eq!(result.returns, vec![crate::val::RuntimeVal::Int(515)]);
}
