use crate::val::NativeArgs;
use crate::vm::compiler::builder::FunctionBuilder;

use super::{Op, Val};

fn native_identity(args: NativeArgs<'_>, _ctx: &mut crate::vm::VmContext) -> anyhow::Result<Val> {
    Ok(args.get(0).cloned().unwrap_or(Val::Nil))
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
