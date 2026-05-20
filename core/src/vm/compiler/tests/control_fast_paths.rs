use super::{Op, Val, parse_compile_and_run};

#[test]
fn conditional_branch_lowers_to_fused_typed_branch() {
    let (function, _ctx, result) = parse_compile_and_run(
        r#"
        let n = 3;
        if n > 1 {
            return 10;
        }
        return 20;
        "#,
    );

    assert_eq!(result.expect("vm exec"), Val::Int(10));
    assert!(
        function
            .code
            .iter()
            .any(|op| matches!(op, Op::CmpGtImmJmp { imm: 1, .. })),
        "typed integer condition should fuse compare and branch in {:?}",
        function.code
    );
}
