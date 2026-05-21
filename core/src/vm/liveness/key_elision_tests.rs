use super::*;

fn string_int_fact() -> PerfStringIntKeyFact {
    PerfStringIntKeyFact {
        prefix_key: 0,
        suffix_reg: 1,
    }
}

#[test]
fn key_facts_track_string_int_key_registers() {
    let consts = vec![Val::from_str("sku-"), Val::Int(42)];
    let mut facts = PerformanceFacts::default();
    let code = vec![
        Op::LoadK(0, 0),
        Op::LoadK(1, 1),
        Op::StrConcatToStr(2, 0, 1),
        Op::MapGetDynamic(3, 4, 2),
        Op::Ret { base: 3, retc: 1 },
    ];

    annotate_control_flow_facts(&mut facts, &code);
    annotate_key_facts(&mut facts, &code, &consts, 5);

    assert_eq!(
        facts.known_key(2).and_then(|fact| fact.string_int),
        Some(string_int_fact())
    );
    assert_eq!(
        facts.known_key(3).and_then(|fact| fact.string_int),
        Some(string_int_fact())
    );
}

#[test]
fn key_facts_track_string_int_key_when_concat_overwrites_prefix() {
    let consts = vec![Val::from_str("sku-"), Val::Int(42)];
    let mut facts = PerformanceFacts::default();
    let code = vec![
        Op::LoadK(0, 0),
        Op::LoadK(1, 1),
        Op::StrConcatToStr(0, 0, 1),
        Op::MapGetDynamic(2, 3, 0),
        Op::Ret { base: 2, retc: 1 },
    ];

    annotate_control_flow_facts(&mut facts, &code);
    annotate_key_facts(&mut facts, &code, &consts, 4);

    assert_eq!(
        facts.known_key(3).and_then(|fact| fact.string_int),
        Some(string_int_fact())
    );
}

#[test]
fn key_facts_propagate_string_int_key_through_upsert_diamond() {
    let consts = vec![Val::from_str("sku-"), Val::Int(42), Val::Int(1), Val::Nil];
    let mut facts = PerformanceFacts::default();
    let code = vec![
        Op::LoadK(0, 0),
        Op::LoadK(1, 1),
        Op::LoadK(5, 2),
        Op::StrConcatToStr(2, 0, 1),
        Op::MapGetDynamic(3, 4, 2),
        Op::CmpEq(6, 3, 7),
        Op::BoolBranch(6, 3),
        Op::MapSet { map: 4, key: 2, val: 5 },
        Op::Jmp(3),
        Op::AddIntImm(8, 3, 1),
        Op::MapSetMove { map: 4, key: 2, val: 8 },
        Op::Ret { base: 3, retc: 1 },
    ];

    annotate_control_flow_facts(&mut facts, &code);
    annotate_key_facts(&mut facts, &code, &consts, 9);

    assert_eq!(
        facts.known_key(7).and_then(|fact| fact.string_int),
        Some(string_int_fact())
    );
    assert_eq!(
        facts.known_key(10).and_then(|fact| fact.string_int),
        Some(string_int_fact())
    );
}

#[test]
fn dead_write_facts_mark_string_int_key_materialization_for_fact_consumers() {
    let consts = vec![Val::from_str("sku-"), Val::Int(42), Val::Int(1)];
    let mut facts = PerformanceFacts::default();
    let code = vec![
        Op::LoadK(0, 0),
        Op::LoadK(1, 1),
        Op::LoadK(5, 2),
        Op::StrConcatToStr(2, 0, 1),
        Op::MapGetDynamic(3, 4, 2),
        Op::MapSet { map: 4, key: 2, val: 5 },
        Op::Ret { base: 3, retc: 1 },
    ];

    annotate_control_flow_facts(&mut facts, &code);
    annotate_key_facts(&mut facts, &code, &consts, 6);
    annotate_dead_write_facts(&mut facts, &code, 6);

    assert!(facts.is_dead_write(3));
}

#[test]
fn dead_write_facts_mark_string_int_key_materialization_for_upsert_diamond() {
    let consts = vec![Val::from_str("sku-"), Val::Int(42), Val::Int(1), Val::Nil];
    let mut facts = PerformanceFacts::default();
    let code = vec![
        Op::LoadK(0, 0),
        Op::LoadK(1, 1),
        Op::LoadK(5, 2),
        Op::StrConcatToStr(2, 0, 1),
        Op::MapGetDynamic(3, 4, 2),
        Op::CmpEq(6, 3, 7),
        Op::BoolBranch(6, 3),
        Op::MapSet { map: 4, key: 2, val: 5 },
        Op::Jmp(3),
        Op::AddIntImm(8, 3, 1),
        Op::MapSetMove { map: 4, key: 2, val: 8 },
        Op::Ret { base: 3, retc: 1 },
    ];

    annotate_control_flow_facts(&mut facts, &code);
    annotate_key_facts(&mut facts, &code, &consts, 9);
    annotate_dead_write_facts(&mut facts, &code, 9);

    assert!(facts.is_dead_write(3));
}

#[test]
fn dead_write_facts_keep_string_int_key_materialization_for_general_reads() {
    let consts = vec![Val::from_str("sku-"), Val::Int(42)];
    let mut facts = PerformanceFacts::default();
    let code = vec![
        Op::LoadK(0, 0),
        Op::LoadK(1, 1),
        Op::StrConcatToStr(2, 0, 1),
        Op::MapGetDynamic(3, 4, 2),
        Op::Ret { base: 2, retc: 1 },
    ];

    annotate_control_flow_facts(&mut facts, &code);
    annotate_key_facts(&mut facts, &code, &consts, 5);
    annotate_dead_write_facts(&mut facts, &code, 5);

    assert!(!facts.is_dead_write(2));
}
