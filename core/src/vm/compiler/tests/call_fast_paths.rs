use crate::val::NativeArgs;
use crate::vm::compiler::builder::FunctionBuilder;

use super::{Op, Val, parse_compile_and_run};

fn native_identity(args: NativeArgs<'_>, _ctx: &mut crate::vm::VmContext) -> anyhow::Result<Val> {
    Ok(args.get(0).cloned().unwrap_or(Val::Nil))
}

#[test]
fn expression_only_known_call_inlines_without_call_window_moves() {
    let source = r#"
        fn add_one(n) {
            return n + 1;
        }
        return add_one(41);
    "#;
    let (function, _ctx, result) = parse_compile_and_run(source);

    assert_eq!(result.expect("vm exec"), Val::Int(42));
    assert!(
        function
            .code
            .iter()
            .all(|op| !matches!(op, Op::CallClosureExact { .. } | Op::Call { .. } | Op::Move(_, _))),
        "expression-only known call should inline without call-window moves in {:?}",
        function.code
    );
}

#[test]
fn positional_call_selector_uses_native_fast_opcode_for_known_native() {
    let mut builder = FunctionBuilder::new();

    builder.emit_positional_call(0, 1, 1, 1, Some(&Val::RustFastFunction(native_identity)));

    assert!(
        matches!(
            builder.code.as_slice(),
            [Op::CallNativeFast {
                f: 0,
                base: 1,
                argc: 1,
                retc: 1
            }]
        ),
        "expected known native positional call to use CallNativeFast in {:?}",
        builder.code
    );
}
