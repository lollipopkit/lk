use super::parse_compile_and_run;
use crate::val::Val;
use crate::vm::{Op, rk_is_const};

#[test]
fn template_literal_chunks_use_str_concat_known_cap() {
    let source = r#"
        let name = "lk";
        return "hello ${name} vm";
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::from_str("hello lk vm"));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::Add(_, _, rhs) if rk_is_const(*rhs))),
        "expected template literal chunks to use RK const Add in {:?}",
        function.code
    );
}

#[test]
fn constant_string_len_lowers_to_integer_const() {
    let source = r#"
        return "/admin/users".len();
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(12));
    assert!(
        function
            .code
            .iter()
            .all(|op| !matches!(op, Op::StrLen { .. } | Op::Len { .. })),
        "constant string len should not emit runtime len op: {:?}",
        function.code
    );
    assert!(
        function
            .code
            .iter()
            .all(|op| !matches!(op, Op::LoadK(_, kidx) if function.consts[*kidx as usize].as_str().is_some())),
        "constant string len should not load the receiver string: {:?}",
        function.code
    );
}

#[test]
fn split_join_same_separator_len_preserves_constant_len() {
    let source = r#"
        return "a|bb|ccc".split("|").join("|").len();
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(8));
    assert!(
        function
            .code
            .iter()
            .all(|op| !matches!(op, Op::StrLen { .. } | Op::Len { .. })),
        "split/join identity len should fold to a const: {:?}",
        function.code
    );
}

#[test]
fn string_for_in_uses_typed_len_and_index() {
    let source = r#"
        let s = "tenant-${7}-order";
        let total = 0;
        for ch in s {
            total += ch.len();
        }
        return total;
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(14));
    assert!(
        function.code.iter().any(|op| matches!(op, Op::StrLen { .. })),
        "expected string for-in to use StrLen in {:?}",
        function.code
    );
    assert!(
        function.code.iter().any(|op| matches!(op, Op::StrIndex(_, _, _))),
        "expected string for-in to use StrIndex in {:?}",
        function.code
    );
    assert!(
        function.code.iter().all(|op| !matches!(op, Op::ToIter { .. })),
        "facts-proven string for-in should avoid ToIter in {:?}",
        function.code
    );
    assert!(
        function.code.iter().all(|op| !matches!(op, Op::LoadLocal(_, _))),
        "facts-proven string for-in should reuse the source register instead of copying a local in {:?}",
        function.code
    );
}
