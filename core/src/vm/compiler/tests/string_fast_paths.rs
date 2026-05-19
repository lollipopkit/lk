use super::parse_compile_and_run;
use crate::{val::Val, vm::Op};

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
            .any(|op| matches!(op, Op::StrConcatKnownCap(_, _, _))),
        "expected template literal chunks to lower to StrConcatKnownCap in {:?}",
        function.code
    );
}
